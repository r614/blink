//! [interp] Matinee data (`InterpData` / Dishonored's `MatineeData`) and its evaluation.
//!
//! Dishonored keeps every matinee in a `MatineeData` object (`InterpData.m_Data`, often a
//! separate "soiree" package cooked into the level, e.g. `Boyle_SamuelBoatIntro`), whose
//! `m_RunData` holds `InterpLength` and `InterpGroups`. A group (`InterpGroup`, the AI group
//! `InterpGroupAI` with its stage mark, the player group `InterpGroupPlayer`, the director
//! group `InterpGroupDirector`) is bound by name to the variable link of the same name on the
//! `SeqAct_Interp` that plays it, and owns tracks.
//!
//! This module is the data model (every track class Boyle's matinees use), the loader (feature
//! `load`), and the evaluation the engine performs (research/dishonored/CUTSCENES.md):
//! - `FInterpCurve::Eval` (constant, linear, Hermite with the stored tangents);
//! - `InterpTrackMove` world / relative-to-initial transforms (Euler track in degrees);
//! - `InterpTrackAnimControl::GetAnimForTime` and Arkane's stretched variant
//!   (`InterpTrackStretchAnimControl`, 0x00905740: piecewise mapping through the sequence's
//!   soiree markers, the End segment looping);
//! - director cuts, fade / slomo / float curves, discrete keys (events, toggles, visibility,
//!   AkEvents, dialog blurbs, attachments);
//! - the soiree control of the playback position (`InterpTrackInstSoireeControl` 0x00900a40 /
//!   0x00900480 / 0x009005e0, dialog waits 0x00c735c0) in [`PlayState::step`].
//!
//! Nothing here depends on the game: the Kismet host (server) and the runtime cutscene module
//! (presentation) both evaluate the same data.

use glam::{Mat3, Quat, Vec3};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------------
// Curves

/// `EInterpCurveMode`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum CurveMode {
    #[default]
    Linear,
    CurveAuto,
    Constant,
    CurveUser,
    CurveBreak,
    CurveAutoClamped,
}

impl CurveMode {
    pub fn from_name(s: &str) -> Self {
        match s {
            "CIM_Constant" => CurveMode::Constant,
            "CIM_CurveAuto" => CurveMode::CurveAuto,
            "CIM_CurveUser" => CurveMode::CurveUser,
            "CIM_CurveBreak" => CurveMode::CurveBreak,
            "CIM_CurveAutoClamped" => CurveMode::CurveAutoClamped,
            _ => CurveMode::Linear,
        }
    }
}

/// `FInterpCurvePoint<T>`: tangents are stored (the editor computes the automatic ones).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct CurvePoint<T> {
    pub t: f32,
    pub v: T,
    pub arrive: T,
    pub leave: T,
    pub mode: CurveMode,
}

pub trait CurveValue: Copy + Default {
    fn lerp(a: Self, b: Self, t: f32) -> Self;
    /// `a * x + b * y + c * z + d * w`.
    fn combine(a: Self, x: f32, b: Self, y: f32, c: Self, z: f32, d: Self, w: f32) -> Self;
}

impl CurveValue for f32 {
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        a + (b - a) * t
    }
    fn combine(a: Self, x: f32, b: Self, y: f32, c: Self, z: f32, d: Self, w: f32) -> Self {
        a * x + b * y + c * z + d * w
    }
}

impl CurveValue for [f32; 3] {
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        [f32::lerp(a[0], b[0], t), f32::lerp(a[1], b[1], t), f32::lerp(a[2], b[2], t)]
    }
    fn combine(a: Self, x: f32, b: Self, y: f32, c: Self, z: f32, d: Self, w: f32) -> Self {
        [0, 1, 2].map(|i| f32::combine(a[i], x, b[i], y, c[i], z, d[i], w))
    }
}

/// `FInterpCurve<T>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Curve<T> {
    pub points: Vec<CurvePoint<T>>,
}

impl<T: CurveValue> Curve<T> {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// `FInterpCurve::Eval`: before the first point its value, after the last point its value;
    /// between points by the mode of the earlier point (constant holds it, linear lerps, the
    /// curve modes use the cubic Hermite with the leave / arrive tangents scaled by the gap).
    pub fn eval(&self, t: f32, default: T) -> T {
        let p = &self.points;
        let n = p.len();
        if n == 0 {
            return default;
        }
        if n < 2 || t <= p[0].t {
            return p[0].v;
        }
        if t >= p[n - 1].t {
            return p[n - 1].v;
        }
        for i in 1..n {
            if t < p[i].t {
                let a = &p[i - 1];
                let b = &p[i];
                let diff = b.t - a.t;
                if diff > 0.0 && a.mode != CurveMode::Constant {
                    let alpha = (t - a.t) / diff;
                    if a.mode == CurveMode::Linear {
                        return T::lerp(a.v, b.v, alpha);
                    }
                    return cubic(a.v, scale(a.leave, diff), b.v, scale(b.arrive, diff), alpha);
                }
                return a.v;
            }
        }
        p[n - 1].v
    }
}

fn scale<T: CurveValue>(v: T, s: f32) -> T {
    T::combine(v, s, v, 0.0, v, 0.0, v, 0.0)
}

/// `CubicInterp(P0, T0, P1, T1, A)` (Hermite basis).
fn cubic<T: CurveValue>(p0: T, t0: T, p1: T, t1: T, a: f32) -> T {
    let a2 = a * a;
    let a3 = a2 * a;
    T::combine(p0, 2.0 * a3 - 3.0 * a2 + 1.0, t0, a3 - 2.0 * a2 + a, t1, a3 - a2, p1, -2.0 * a3 + 3.0 * a2)
}

pub type CurveF = Curve<f32>;
pub type CurveV = Curve<[f32; 3]>;

// ---------------------------------------------------------------------------------------------
// Rotations

/// `FRotationMatrix` from degrees (pitch, yaw, roll), as a column-vector matrix (`M * v`
/// equals UE3's `v * M`): X axis (cp cy, cp sy, sp).
pub fn rot_matrix_deg(pitch: f32, yaw: f32, roll: f32) -> Mat3 {
    let (sp, cp) = pitch.to_radians().sin_cos();
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sr, cr) = roll.to_radians().sin_cos();
    let x = Vec3::new(cp * cy, cp * sy, sp);
    let y = Vec3::new(sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp);
    let z = Vec3::new(-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp);
    Mat3::from_cols(x, y, z)
}

/// UE3 rotator (65536 units per turn) -> matrix.
pub fn rot_matrix_units(r: [i32; 3]) -> Mat3 {
    let k = 360.0 / 65536.0;
    rot_matrix_deg(r[0] as f32 * k, r[1] as f32 * k, r[2] as f32 * k)
}

/// `FMatrix::Rotator` in degrees (pitch, yaw, roll).
pub fn matrix_rotator_deg(m: &Mat3) -> [f32; 3] {
    let x = m.x_axis;
    let pitch = x.z.atan2((x.x * x.x + x.y * x.y).sqrt()).to_degrees();
    let yaw = x.y.atan2(x.x).to_degrees();
    let sy = rot_matrix_deg(pitch, yaw, 0.0).y_axis;
    let roll = m.z_axis.dot(sy).atan2(m.y_axis.dot(sy)).to_degrees();
    [pitch, yaw, roll]
}

/// Position + rotation of an actor (Dishonored world units).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Xform {
    pub pos: Vec3,
    pub rot: Mat3,
}

impl Default for Xform {
    fn default() -> Self {
        Xform { pos: Vec3::ZERO, rot: Mat3::IDENTITY }
    }
}

impl Xform {
    pub fn new(pos: [f32; 3], rot: [i32; 3]) -> Self {
        Xform { pos: Vec3::from(pos), rot: rot_matrix_units(rot) }
    }
    /// `self * child` (child expressed in this frame).
    pub fn then(&self, child: &Xform) -> Xform {
        Xform { pos: self.pos + self.rot * child.pos, rot: self.rot * child.rot }
    }
    pub fn yaw_deg(&self) -> f32 {
        matrix_rotator_deg(&self.rot)[1]
    }
    pub fn quat(&self) -> Quat {
        Quat::from_mat3(&self.rot)
    }
}

// ---------------------------------------------------------------------------------------------
// Data model

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Matinee {
    /// Package and export of the object the data was read from (the runtime reloads it).
    pub package: String,
    pub export: u32,
    pub path: String,
    pub length: f32,
    pub groups: Vec<Group>,
    /// `RuntimeMatineeData.m_ConversationNodeGUID` (a soiree's conversation node).
    pub conversation_node: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub enum GroupKind {
    #[default]
    Plain,
    Folder,
    Director,
    /// `InterpGroupAI`.
    Ai {
        stage_mark: String,
        snap_to_root: bool,
        lock_requests: bool,
        can_interrupt: bool,
        detached: bool,
    },
    /// `InterpGroupPlayer`.
    Player {
        stage_mark: String,
        /// `m_fTimeToBlendToStageMark` (class default 0.2 s).
        blend_time: f32,
        snap_location: bool,
        snap_rotation: bool,
        disable_collision: bool,
        camera_to_dialog: bool,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Group {
    pub name: String,
    pub kind: GroupKind,
    /// `bIsParented` (shown under the folder above it in the editor).
    pub parented: bool,
    /// `GroupAnimSets`: (package, object path).
    pub anim_sets: Vec<(String, String)>,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub class: String,
    pub title: String,
    pub disabled: bool,
    pub kind: TrackKind,
}

/// `EInterpTrackMoveFrame`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum MoveFrame {
    #[default]
    World,
    RelativeToInitial,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AnimKey {
    pub start: f32,
    pub name: String,
    pub start_offset: f32,
    pub end_offset: f32,
    pub rate: f32,
    pub looping: bool,
    pub reverse: bool,
    /// Stretched keys: `SoireeOffsets[ESoireeMarkers]` (Accent, Apex, Loop, End, In, Out).
    pub soiree: [f32; 6],
    /// `InterpTrackStretchAnimKeyProperties.m_bDisableCollision`.
    pub disable_collision: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DirectorCut {
    pub time: f32,
    pub transition: f32,
    pub group: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DialogKey {
    pub start: f32,
    pub length: f32,
    pub blurb: String,
    pub alts: Vec<String>,
    pub wait: bool,
}

/// Keys spanning a time range aimed at a target (look at, face to, locomotion).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SpanKey {
    pub start: f32,
    pub length: f32,
    /// `ENPCControlTargetType`: 0 player, 1 self, 2 a group by `target`.
    pub target_type: u8,
    pub target: String,
    /// Look at: `ELookAtInfluence` (0 eyes, 1 head, 2 torso). Locomotion: modifier.
    pub influence: u8,
    /// Locomotion: `m_SpeedIdxName` / `m_EndSpeedIdxName`; look at: bone or socket.
    pub speed: String,
    pub end_speed: String,
    pub wait_goal: bool,
    pub stop_distance: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AttachKey {
    pub time: f32,
    pub detach: bool,
    pub bone: String,
    pub offset: [f32; 3],
    pub rotation: [i32; 3],
    pub use_offset: bool,
    pub use_rotation: bool,
    pub hard: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SoireeKey {
    pub start: f32,
    pub length: f32,
    /// `ESoireeControlType`: 0 loop, 1 pause.
    pub pause: bool,
    /// `ESoireeBreakMode`: false at the next loop, true immediately.
    pub break_now: bool,
    /// `m_LoopCount` (0 = until broken).
    pub loops: i32,
    pub pin: String,
    pub blend_out: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub enum TrackKind {
    Move {
        pos: CurveV,
        euler: CurveV,
        frame: MoveFrame,
        /// `EInterpTrackMoveRotMode`: 0 keyframed, 1 look at group, 2 ignore.
        rot_mode: u8,
        look_at: String,
    },
    /// `InterpTrackAnimControl` / `InterpTrackStretchAnimControl`; `weight` is the track's
    /// float curve (the slot channel weight).
    Anim { slot: String, keys: Vec<AnimKey>, weight: CurveF, stretch: bool },
    Director { cuts: Vec<DirectorCut> },
    Fade { curve: CurveF, persist: bool },
    Slomo { curve: CurveF },
    FloatProp { property: String, curve: CurveF },
    Event { keys: Vec<(f32, String)>, forwards: bool, backwards: bool },
    /// `InterpTrackAkEvent`: (time, event name, stop flag).
    AkEvent { keys: Vec<(f32, String, bool)> },
    /// `ETrackToggleAction`: 0 off, 1 on, 2 toggle, 3 trigger.
    Toggle { keys: Vec<(f32, u8)> },
    /// `EVisibilityTrackAction`: 0 hide, 1 show, 2 toggle.
    Visibility { keys: Vec<(f32, u8)> },
    Dialog { keys: Vec<DialogKey> },
    LookAt { keys: Vec<SpanKey>, weight: CurveF },
    FaceTo { keys: Vec<SpanKey>, weight: CurveF },
    Locomotion { keys: Vec<SpanKey> },
    Attachment { target: String, keys: Vec<AttachKey>, detach_on_interrupt: bool },
    /// `DisInterpTrackAllowPlayerLook`: (time, yaw min, yaw max, pitch min, pitch max) degrees.
    AllowPlayerLook { keys: Vec<(f32, [f32; 4])> },
    SoireeControl { keys: Vec<SoireeKey> },
    /// `InterpTrackFaceFX`: (start, FaceFX group, sequence).
    FaceFx { keys: Vec<(f32, String, String)> },
    #[default]
    Other,
}

impl Matinee {
    pub fn group(&self, name: &str) -> Option<&Group> {
        self.groups.iter().find(|g| g.name.eq_ignore_ascii_case(name))
    }

    /// Event track keys (time, name, forwards, backwards), sorted by time.
    pub fn event_keys(&self) -> Vec<(f32, String, bool, bool)> {
        let mut v = Vec::new();
        for t in self.groups.iter().flat_map(|g| &g.tracks) {
            if t.disabled {
                continue;
            }
            if let TrackKind::Event { keys, forwards, backwards } = &t.kind {
                v.extend(keys.iter().map(|(time, n)| (*time, n.clone(), *forwards, *backwards)));
            }
        }
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v
    }

    /// The director group's track of a kind.
    pub fn director_tracks(&self) -> impl Iterator<Item = &Track> {
        self.groups.iter().filter(|g| g.kind == GroupKind::Director).flat_map(|g| g.tracks.iter()).filter(|t| !t.disabled)
    }

    /// Fade amount at `t` (0 clear, 1 black) from the director's fade track, if any.
    pub fn fade(&self, t: f32) -> Option<f32> {
        self.director_tracks().find_map(|tr| match &tr.kind {
            TrackKind::Fade { curve, .. } => Some(curve.eval(t, 0.0)),
            _ => None,
        })
    }

    /// Time dilation at `t` from the director's slomo track, if any.
    pub fn slomo(&self, t: f32) -> Option<f32> {
        self.director_tracks().find_map(|tr| match &tr.kind {
            TrackKind::Slomo { curve } => Some(curve.eval(t, 1.0)),
            _ => None,
        })
    }

    /// Director cut at `t`: (camera group viewed, index of the cut). None before the first cut
    /// (the player's own view, `InterpTrackDirector::GetViewedGroupName`).
    pub fn director_cut(&self, t: f32) -> Option<(&DirectorCut, usize)> {
        self.director_tracks().find_map(|tr| match &tr.kind {
            TrackKind::Director { cuts } => cuts.iter().enumerate().rev().find(|(_, c)| c.time <= t).map(|(i, c)| (c, i)),
            _ => None,
        })
    }

    pub fn has_player_group(&self) -> bool {
        self.groups.iter().any(|g| matches!(g.kind, GroupKind::Player { .. }))
    }

    /// Player look limits at `t` (`DisInterpTrackAllowPlayerLook`, last key at or before t).
    pub fn player_look(&self, t: f32) -> Option<[f32; 4]> {
        let g = self.groups.iter().find(|g| matches!(g.kind, GroupKind::Player { .. }))?;
        g.tracks.iter().find_map(|tr| match &tr.kind {
            TrackKind::AllowPlayerLook { keys } => keys.iter().rev().find(|k| k.0 <= t).map(|k| k.1),
            _ => None,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Track evaluation

impl TrackKind {
    /// Move track value at `t`: relative position and Euler rotation (degrees: X roll,
    /// Y pitch, Z yaw, `FRotator::MakeFromEuler`).
    pub fn move_key(&self, t: f32) -> Option<(Vec3, Mat3)> {
        let TrackKind::Move { pos, euler, .. } = self else { return None };
        let p = pos.eval(t, [0.0; 3]);
        let e = euler.eval(t, [0.0; 3]);
        Some((Vec3::from(p), rot_matrix_deg(e[1], e[2], e[0])))
    }

    /// `InterpTrackMove::ComputeWorldSpaceKeyTransform`: world keys as they are; relative
    /// keys composed with the actor's transform when the matinee initialised (`InitialTM`).
    pub fn move_world(&self, t: f32, initial: &Xform) -> Option<Xform> {
        let TrackKind::Move { frame, rot_mode, .. } = self else { return None };
        let (p, r) = self.move_key(t)?;
        let key = Xform { pos: p, rot: if *rot_mode == 2 { Mat3::IDENTITY } else { r } };
        let mut x = match frame {
            MoveFrame::World => key,
            MoveFrame::RelativeToInitial => initial.then(&key),
        };
        if *rot_mode == 2 {
            x.rot = initial.rot;
        }
        Some(x)
    }
}

/// What an AnimSequence contributes to anim key evaluation: its length and soiree markers
/// (`AnimNotify_SoireeAccent/Apex/Loop/End/In/Out` notifies: time, marker index).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeqInfo {
    pub length: f32,
    pub markers: Vec<(f32, u8)>,
}

/// Marker index of a soiree notify class (`ESoireeMarkers`), 6 = not a marker.
pub fn soiree_marker(class: &str) -> u8 {
    match class {
        "AnimNotify_SoireeAccent" => 0,
        "AnimNotify_SoireeApex" => 1,
        "AnimNotify_SoireeLoop" => 2,
        "AnimNotify_SoireeEnd" => 3,
        "AnimNotify_SoireeIn" => 4,
        "AnimNotify_SoireeOut" => 5,
        _ => 6,
    }
}

/// An anim track's sample at a time.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimSample {
    pub key: usize,
    pub name: String,
    /// Position in the sequence (seconds).
    pub position: f32,
    pub looping: bool,
    /// The key ran past its end (stretched keys report no key then).
    pub finished: bool,
}

/// Active key of an anim track at `t`: the last key starting at or before it (the first key
/// before the first start).
pub fn anim_key_at(keys: &[AnimKey], t: f32) -> Option<usize> {
    if keys.is_empty() {
        return None;
    }
    Some(keys.iter().rposition(|k| k.start <= t).unwrap_or(0))
}

/// `InterpTrackAnimControl::GetAnimForTime` (UE3): before the first key, the first key at its
/// start offset; else `(t - start) * rate`, looping over the trimmed length or clamped to
/// `length - end offset`; reversed keys play from the end.
pub fn anim_at(keys: &[AnimKey], t: f32, seq: &dyn Fn(&str) -> Option<SeqInfo>) -> Option<AnimSample> {
    let i = anim_key_at(keys, t)?;
    let k = &keys[i];
    if t < k.start {
        return Some(AnimSample { key: i, name: k.name.clone(), position: k.start_offset, looping: k.looping, finished: false });
    }
    let len = seq(&k.name).map(|s| s.length).unwrap_or(0.0);
    let mut p = (t - k.start) * k.rate;
    let trimmed = (len - (k.start_offset + k.end_offset)).max(0.01);
    let mut finished = false;
    if k.looping {
        p = p.rem_euclid(trimmed) + k.start_offset;
    } else {
        let end = (len - k.end_offset).max(0.0);
        if p + k.start_offset >= end {
            finished = true;
        }
        p = (p + k.start_offset).clamp(0.0, end + 1e-4);
    }
    if k.reverse {
        p = (len - p).max(0.0);
    }
    Some(AnimSample { key: i, name: k.name.clone(), position: p, looping: k.looping, finished })
}

/// Arkane's stretched key (0x00905740). The key covers the sequence from its start offset to
/// `length - end offset` in "stretched" seconds (times the play rate). Each soiree marker sits
/// at its time plus the key's offset for that marker; between markers the stretched time maps
/// linearly onto the sequence, except the segment ending at the End marker, which loops a
/// whole number of times (`round(stretched / original)`, at least once) to fill its span.
pub fn stretch_at(keys: &[AnimKey], t: f32, seq: &dyn Fn(&str) -> Option<SeqInfo>) -> Option<AnimSample> {
    let i = anim_key_at(keys, t)?;
    let k = &keys[i];
    if t < k.start {
        return Some(AnimSample { key: i, name: k.name.clone(), position: 0.0, looping: false, finished: false });
    }
    let Some(info) = seq(&k.name) else {
        return Some(AnimSample { key: i, name: k.name.clone(), position: 0.0, looping: false, finished: false });
    };
    let len = info.length;
    let end = len - k.end_offset + 1e-4;
    let mut p = (t - k.start) * k.rate + k.start_offset;
    let mut finished = false;
    if p < k.start_offset {
        p = k.start_offset;
    } else if p >= end {
        p = end;
        finished = true;
    }
    let mut seg_s = k.start_offset;
    if k.reverse {
        seg_s = k.end_offset;
        p = len - p;
    }
    let mut seg_o = 0.0f32;
    let mut markers = info.markers.clone();
    markers.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (m, ty) in markers {
        if ty > 5 {
            continue;
        }
        let s = m + k.soiree[ty as usize];
        if p < s {
            if ty == 3 {
                let orig = m - seg_o;
                let span = s - seg_s;
                let n = if orig > 0.0 { ((span / orig) + 0.5).max(1.0).floor() } else { 1.0 };
                let period = span / n;
                let pos = if period > 0.0 { (p - seg_s).rem_euclid(period) * (orig / period) + seg_o } else { seg_o };
                return Some(AnimSample { key: i, name: k.name.clone(), position: pos, looping: true, finished });
            }
            let pos = if s - seg_s != 0.0 { (m - seg_o) / (s - seg_s) * (p - seg_s) + seg_o } else { m };
            return Some(AnimSample { key: i, name: k.name.clone(), position: pos, looping: false, finished });
        }
        seg_o = m;
        seg_s = s;
    }
    let tail = (len - k.end_offset) - seg_s;
    let scale = if tail != 0.0 { (len - seg_o) / tail } else { 1.0 };
    Some(AnimSample { key: i, name: k.name.clone(), position: (p - seg_s) * scale + seg_o, looping: false, finished })
}

impl Track {
    /// Anim track sample at `t` and its weight (the float curve; 1 without one).
    pub fn anim(&self, t: f32, seq: &dyn Fn(&str) -> Option<SeqInfo>) -> Option<(AnimSample, f32)> {
        let TrackKind::Anim { keys, weight, stretch, .. } = &self.kind else { return None };
        let s = if *stretch { stretch_at(keys, t, seq)? } else { anim_at(keys, t, seq)? };
        let w = if weight.is_empty() { 1.0 } else { weight.eval(t, 0.0).clamp(0.0, 1.0) };
        Some((s, w))
    }
}

/// Keys crossed by a step (forwards: `from < key <= to`; backwards: `to <= key < from`).
pub fn crossed(time: f32, from: f32, to: f32) -> bool {
    if to >= from { time > from && time <= to } else { time < from && time >= to }
}

/// The last key at or before `t` (discrete state tracks: attachment, toggle, visibility).
pub fn last_key<T>(keys: &[T], t: f32, time: impl Fn(&T) -> f32) -> Option<usize> {
    keys.iter().rposition(|k| time(k) <= t)
}

// ---------------------------------------------------------------------------------------------
// Playback control (soiree loops, pauses, dialog waits)

/// Per-matinee soiree control state (`InterpTrackInstSoireeControl`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayState {
    /// (track, key) of the loop being played, and the loops done.
    pub current: Option<(usize, usize)>,
    pub loops: i32,
    /// Pins broken (`SCT_<pin>` inputs pulsed), lowercase.
    pub broken: Vec<String>,
    /// Time held on a pause key.
    pub paused: f32,
    /// Blurbs still speaking (dialog waits).
    pub speaking: Vec<String>,
}

/// Result of a playback step after soiree control.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub pos: f32,
    /// The position jumped (loop back or break): discrete keys are not fired across it.
    pub jumped: bool,
}

impl PlayState {
    pub fn break_pin(&mut self, pin: &str) {
        let p = pin.to_ascii_lowercase();
        if !self.broken.contains(&p) {
            self.broken.push(p);
        }
    }

    fn is_broken(&self, pin: &str) -> bool {
        !pin.is_empty() && self.broken.iter().any(|b| b.eq_ignore_ascii_case(pin))
    }

    /// Forward step from `old` to `new` (seconds), applying the soiree control tracks:
    /// - entering a loop key (`old <= start < new`) makes it current (0x009005e0);
    /// - a loop key broken with `BreakImmediately` while inside it jumps past its end
    ///   (0x00900a40);
    /// - passing the end of the current loop key wraps back by its length while it is not
    ///   broken and its loop count is not reached (0x00900480, 0x006340f0);
    /// - a pause key holds the position at its start until broken;
    /// - a dialog key with `WaitForEndOfBlurb` holds just before its end while its blurb speaks
    ///   (0x00c735c0).
    pub fn step(&mut self, m: &Matinee, old: f32, new: f32) -> Step {
        let mut pos = new;
        let mut jumped = false;
        let tracks: Vec<&Track> = m.groups.iter().flat_map(|g| g.tracks.iter()).filter(|t| !t.disabled).collect();
        // Holds (pause, break immediately, dialog wait).
        for t in &tracks {
            match &t.kind {
                TrackKind::SoireeControl { keys } => {
                    for k in keys {
                        let len = if k.pause { 0.0 } else { k.length };
                        if !(old <= k.start + len && k.start < pos) {
                            continue;
                        }
                        if !k.pause {
                            if k.start + len >= pos && self.is_broken(&k.pin) && k.break_now {
                                pos = k.start + len + 1e-4;
                                jumped = true;
                                self.current = None;
                            }
                        } else if k.start < pos && !self.is_broken(&k.pin) {
                            self.paused += pos - old;
                            pos = k.start;
                        } else {
                            self.paused = 0.0;
                        }
                        break;
                    }
                }
                TrackKind::Dialog { keys } => {
                    for k in keys.iter().rev() {
                        let end = k.start + k.length;
                        if k.wait && old <= end && end <= pos && self.speaking.iter().any(|b| b.eq_ignore_ascii_case(&k.blurb)) {
                            pos = end - 1e-4;
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        // Entering a loop key.
        for (ti, t) in tracks.iter().enumerate() {
            if let TrackKind::SoireeControl { keys } = &t.kind {
                if let Some(ki) = keys.iter().rposition(|k| !k.pause && old <= k.start && k.start < pos) {
                    self.current = Some((ti, ki));
                    self.loops = 0;
                }
            }
        }
        // Looping back.
        if let Some((ti, ki)) = self.current {
            if let Some(TrackKind::SoireeControl { keys }) = tracks.get(ti).map(|t| &t.kind) {
                let k = &keys[ki];
                let end = k.start + k.length;
                if pos > end && !self.is_broken(&k.pin) && k.length > 0.0 {
                    let again = if k.loops <= 0 {
                        self.loops += 1;
                        true
                    } else if self.loops < k.loops {
                        self.loops += 1;
                        if self.loops < k.loops {
                            true
                        } else {
                            self.current = None;
                            self.loops = 0;
                            false
                        }
                    } else {
                        false
                    };
                    if again {
                        while pos > end {
                            pos -= k.length;
                        }
                        jumped = true;
                    }
                } else if pos > end {
                    self.current = None;
                }
            } else {
                self.current = None;
            }
        }
        Step { pos, jumped }
    }
}

// ---------------------------------------------------------------------------------------------
// Scenes: groups bound to the world

/// What a group drives: a level actor (Kismet actor index) or a pawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Bound {
    Actor(u32),
    Pawn(u32),
}

/// The world as a scene needs it.
pub trait SceneWorld {
    /// Current transform of a bound object (actors: their level transform or where Kismet
    /// moved them; pawns: position and yaw).
    fn xform(&self, b: Bound) -> Option<Xform>;
    /// Reference-pose bone of a skeletal mesh actor, in actor space.
    fn bone(&self, actor: u32, bone: &str) -> Option<Xform>;
}

/// A matinee playing with its groups bound (`InterpGroupInst`s).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    /// Bound objects per group (same order as `Matinee::groups`).
    pub bound: Vec<Vec<Bound>>,
    /// `InitialTM` of each bound object when the matinee started.
    pub initial: Vec<Vec<Option<Xform>>>,
}

/// What a scene asks of the world at a position.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SceneOut {
    /// Objects placed by move tracks and attachments (and the player at its stage mark).
    pub poses: Vec<(Bound, Xform)>,
    /// Locomotion keys started this step: (pawn, target, speed name, stop distance).
    pub gotos: Vec<(u32, Bound, String, f32)>,
    /// Face-to keys started this step: (pawn, target).
    pub faces: Vec<(u32, Bound)>,
    /// Visibility keys crossed: (object, shown).
    pub visibility: Vec<(Bound, bool)>,
}

impl Scene {
    pub fn new(m: &Matinee, bound: Vec<Vec<Bound>>, world: &dyn SceneWorld) -> Self {
        let initial = bound.iter().map(|v| v.iter().map(|b| world.xform(*b)).collect()).collect();
        let mut bound = bound;
        bound.resize(m.groups.len(), Vec::new());
        Scene { bound, initial }
    }

    fn group_index(m: &Matinee, name: &str) -> Option<usize> {
        m.groups.iter().position(|g| g.name.eq_ignore_ascii_case(name))
    }

    /// The attachment of group `gi` at `t`: (parent group, key) of the last key at or before
    /// `t` on any attachment track targeting it.
    fn attachment(m: &Matinee, gi: usize, t: f32) -> Option<(usize, &AttachKey)> {
        let name = &m.groups[gi].name;
        let mut best: Option<(usize, &AttachKey)> = None;
        for (pi, g) in m.groups.iter().enumerate() {
            for tr in g.tracks.iter().filter(|t| !t.disabled) {
                if let TrackKind::Attachment { target, keys, .. } = &tr.kind {
                    if !target.eq_ignore_ascii_case(name) {
                        continue;
                    }
                    if let Some(k) = keys.iter().filter(|k| k.time <= t).last() {
                        if best.is_none_or(|b| b.1.time <= k.time) {
                            best = Some((pi, k));
                        }
                    }
                }
            }
        }
        best
    }

    /// World transform of object `oi` of group `gi` at `t`, when the matinee drives it.
    pub fn xform(&self, m: &Matinee, gi: usize, oi: usize, t: f32, world: &dyn SceneWorld, depth: u32) -> Option<Xform> {
        if depth > 8 {
            return None;
        }
        let g = &m.groups[gi];
        let initial = self.initial.get(gi).and_then(|v| v.get(oi).copied().flatten());
        if let Some(tr) = g.tracks.iter().find(|t| !t.disabled && matches!(t.kind, TrackKind::Move { .. })) {
            return tr.kind.move_world(t, &initial.unwrap_or_default());
        }
        if let Some((pi, k)) = Self::attachment(m, gi, t) {
            if k.detach {
                // Detached: stays where the attachment left it.
                return if k.time > 0.0 { self.xform(m, gi, oi, k.time - 1e-4, world, depth + 1) } else { None };
            }
            let parent = self.xform(m, pi, 0, t, world, depth + 1).or_else(|| self.initial.get(pi).and_then(|v| v.first().copied().flatten()))?;
            let bone = match self.bound.get(pi).and_then(|v| v.first()) {
                Some(Bound::Actor(a)) if !k.bone.is_empty() => world.bone(*a, &k.bone).unwrap_or_default(),
                _ => Xform::default(),
            };
            let rel = Xform { pos: if k.use_offset { Vec3::from(k.offset) } else { Vec3::ZERO }, rot: if k.use_rotation { rot_matrix_units(k.rotation) } else { Mat3::IDENTITY } };
            return Some(parent.then(&bone).then(&rel));
        }
        if let GroupKind::Player { stage_mark, .. } = &g.kind {
            let si = Self::group_index(m, stage_mark)?;
            return self.initial.get(si).and_then(|v| v.first().copied().flatten());
        }
        None
    }

    /// Evaluate the scene at `t` (previous position `prev`, for keys crossed).
    pub fn update(&self, m: &Matinee, prev: f32, t: f32, world: &dyn SceneWorld) -> SceneOut {
        let mut out = SceneOut::default();
        for (gi, g) in m.groups.iter().enumerate() {
            let objs = self.bound.get(gi).cloned().unwrap_or_default();
            for (oi, b) in objs.iter().enumerate() {
                if let Some(x) = self.xform(m, gi, oi, t, world, 0) {
                    out.poses.push((*b, x));
                }
            }
            let target_of = |kind: u8, name: &str, me: Bound| -> Option<Bound> {
                match kind {
                    1 => Some(me),
                    2 => Self::group_index(m, name).and_then(|i| self.bound.get(i)).and_then(|v| v.first().copied()),
                    _ => None,
                }
            };
            for tr in g.tracks.iter().filter(|t| !t.disabled) {
                match &tr.kind {
                    TrackKind::Locomotion { keys } => {
                        for k in keys.iter().filter(|k| crossed(k.start, prev, t) || (prev == 0.0 && k.start == 0.0 && t > 0.0)) {
                            for b in &objs {
                                if let (Bound::Pawn(p), Some(tg)) = (b, target_of(k.target_type, &k.target, *b)) {
                                    out.gotos.push((*p, tg, k.speed.clone(), k.stop_distance));
                                }
                            }
                        }
                    }
                    TrackKind::FaceTo { keys, .. } => {
                        for k in keys.iter().filter(|k| crossed(k.start, prev, t)) {
                            for b in &objs {
                                if let (Bound::Pawn(p), Some(tg)) = (b, target_of(k.target_type, &k.target, *b)) {
                                    out.faces.push((*p, tg));
                                }
                            }
                        }
                    }
                    TrackKind::Visibility { keys } => {
                        for (time, action) in keys.iter().filter(|k| crossed(k.0, prev, t)) {
                            let _ = time;
                            for b in &objs {
                                out.visibility.push((*b, *action == 1));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// Groups whose objects are pawns under matinee control (AI groups and the player group).
    pub fn controlled_pawns(&self, m: &Matinee) -> Vec<u32> {
        let mut v = Vec::new();
        for (gi, g) in m.groups.iter().enumerate() {
            if matches!(g.kind, GroupKind::Ai { .. } | GroupKind::Player { .. }) {
                for b in self.bound.get(gi).into_iter().flatten() {
                    if let Bound::Pawn(p) = b {
                        v.push(*p);
                    }
                }
            }
        }
        v
    }
}

// ---------------------------------------------------------------------------------------------
// Loading (feature `load`)

#[cfg(feature = "load")]
pub use self::load_impl::load;

#[cfg(feature = "load")]
mod load_impl {
    use super::*;
    use ue3_package::properties::{Property, Value as PV, find};
    use ue3_package::{ObjRef, Package};

    fn f(p: &[Property], n: &str, d: f32) -> f32 {
        find(p, n).and_then(|x| x.as_float()).unwrap_or(d)
    }
    fn b(p: &[Property], n: &str, d: bool) -> bool {
        match find(p, n).map(|x| &x.value) {
            Some(PV::Bool(v)) => *v,
            _ => d,
        }
    }
    fn name(p: &[Property], n: &str) -> String {
        match find(p, n).map(|x| &x.value) {
            Some(PV::Name(s)) if s != "None" => s.clone(),
            Some(PV::Str(s)) => s.clone(),
            _ => String::new(),
        }
    }
    fn enum_idx(p: &[Property], n: &str, names: &[&str], d: u8) -> u8 {
        match find(p, n).map(|x| &x.value) {
            Some(PV::Enum(s)) => names.iter().position(|x| x == s).map(|i| i as u8).unwrap_or(d),
            Some(PV::Byte(v)) => *v,
            _ => d,
        }
    }
    fn guid(p: &[Property], n: &str) -> String {
        match find(p, n).map(|x| &x.value) {
            Some(PV::Guid(g)) => format!("{:08X}{:08X}{:08X}{:08X}", g[0], g[1], g[2], g[3]),
            _ => String::new(),
        }
    }
    fn obj_props(pkg: &Package, p: &[Property], n: &str) -> Vec<Property> {
        match find(p, n).and_then(|x| x.as_object()).map(ObjRef::from_index) {
            Some(ObjRef::Export(e)) => pkg.read_object(e).map(|h| h.properties).unwrap_or_default(),
            _ => Vec::new(),
        }
    }
    fn structs(pkg: &Package, p: &[Property], n: &str) -> Vec<Vec<Property>> {
        find(p, n).and_then(|x| x.array_structs(pkg).ok()).unwrap_or_default()
    }

    fn curve_f(pkg: &Package, p: &[Property], n: &str) -> CurveF {
        let Some(PV::Struct(_, inner)) = find(p, n).map(|x| &x.value) else { return CurveF::default() };
        let points = structs(pkg, inner, "Points")
            .iter()
            .map(|q| CurvePoint { t: f(q, "InVal", 0.0), v: f(q, "OutVal", 0.0), arrive: f(q, "ArriveTangent", 0.0), leave: f(q, "LeaveTangent", 0.0), mode: mode(q) })
            .collect();
        Curve { points }
    }
    fn vec(q: &[Property], n: &str) -> [f32; 3] {
        find(q, n).and_then(|x| x.as_vector()).unwrap_or([0.0; 3])
    }
    fn mode(q: &[Property]) -> CurveMode {
        match find(q, "InterpMode").map(|x| &x.value) {
            Some(PV::Enum(s)) => CurveMode::from_name(s),
            Some(PV::Byte(v)) => [CurveMode::Linear, CurveMode::CurveAuto, CurveMode::Constant, CurveMode::CurveUser, CurveMode::CurveBreak, CurveMode::CurveAutoClamped].get(*v as usize).copied().unwrap_or_default(),
            _ => CurveMode::Linear,
        }
    }
    fn curve_v(pkg: &Package, p: &[Property], n: &str) -> CurveV {
        let Some(PV::Struct(_, inner)) = find(p, n).map(|x| &x.value) else { return CurveV::default() };
        let points = structs(pkg, inner, "Points").iter().map(|q| CurvePoint { t: f(q, "InVal", 0.0), v: vec(q, "OutVal"), arrive: vec(q, "ArriveTangent"), leave: vec(q, "LeaveTangent"), mode: mode(q) }).collect();
        Curve { points }
    }

    fn span_keys(pkg: &Package, p: &[Property], n: &str) -> Vec<SpanKey> {
        structs(pkg, p, n)
            .iter()
            .map(|k| {
                let kp = obj_props(pkg, k, "Properties");
                SpanKey {
                    start: f(k, "StartTime", 0.0),
                    length: f(k, "KeyLength", 0.0),
                    target_type: enum_idx(&kp, "m_TargetType", &["ENCTT_Player", "ENCTT_Self", "ENCTT_Target"], 0),
                    target: name(&kp, "m_TargetName"),
                    influence: if find(&kp, "m_LookAtInfluence").is_some() { enum_idx(&kp, "m_LookAtInfluence", &["ELAI_Eyes", "ELAI_Head", "ELAI_Torso"], 2) } else { enum_idx(&kp, "m_Modifier", &["EKLM_Empty", "EKLM_Sword"], 0) },
                    speed: if find(&kp, "m_SpeedIdxName").is_some() { name(&kp, "m_SpeedIdxName") } else { name(&kp, "m_BoneOrSocket") },
                    end_speed: if find(&kp, "m_EndSpeedIdxName").is_some() { name(&kp, "m_EndSpeedIdxName") } else { "Idle".into() },
                    wait_goal: b(&kp, "WaitForGoalReached", true),
                    stop_distance: f(&kp, "m_fTargetStopDistance", 0.0),
                }
            })
            .collect()
    }

    fn track(pkg: &Package, e: usize) -> Track {
        let class = pkg.export_class(e);
        let p = pkg.read_object(e).map(|h| h.properties).unwrap_or_default();
        let kind = match class.as_str() {
            "InterpTrackMove" => TrackKind::Move {
                pos: curve_v(pkg, &p, "PosTrack"),
                euler: curve_v(pkg, &p, "EulerTrack"),
                frame: if enum_idx(&p, "MoveFrame", &["IMF_World", "IMF_RelativeToInitial"], 0) == 1 { MoveFrame::RelativeToInitial } else { MoveFrame::World },
                rot_mode: enum_idx(&p, "RotMode", &["IMR_Keyframed", "IMR_LookAtGroup", "IMR_Ignore"], 0),
                look_at: name(&p, "LookAtGroupName"),
            },
            "InterpTrackAnimControl" | "InterpTrackStretchAnimControl" => {
                let stretch = class == "InterpTrackStretchAnimControl";
                let keys = structs(pkg, &p, "AnimSeqs")
                    .iter()
                    .map(|k| {
                        let mut soiree = [0.0; 6];
                        for q in k.iter().filter(|q| q.name == "SoireeOffsets") {
                            if let (Some(v), true) = (q.as_float(), (0..6).contains(&q.array_index)) {
                                soiree[q.array_index as usize] = v;
                            }
                        }
                        let kp = obj_props(pkg, k, "Properties");
                        AnimKey {
                            start: f(k, "StartTime", 0.0),
                            name: name(k, "AnimSeqName"),
                            start_offset: f(k, "AnimStartOffset", 0.0),
                            end_offset: f(k, "AnimEndOffset", 0.0),
                            rate: f(k, "AnimPlayRate", 0.0),
                            looping: b(k, "bLooping", false),
                            reverse: b(k, "bReverse", false),
                            soiree,
                            disable_collision: b(&kp, "m_bDisableCollision", false),
                        }
                    })
                    .collect();
                TrackKind::Anim { slot: name(&p, "SlotName"), keys, weight: curve_f(pkg, &p, "FloatTrack"), stretch }
            }
            "InterpTrackDirector" => TrackKind::Director {
                cuts: structs(pkg, &p, "CutTrack").iter().map(|k| DirectorCut { time: f(k, "Time", 0.0), transition: f(k, "TransitionTime", 0.0), group: name(k, "TargetCamGroup") }).collect(),
            },
            "InterpTrackFade" => TrackKind::Fade { curve: curve_f(pkg, &p, "FloatTrack"), persist: b(&p, "bPersistFade", false) },
            "InterpTrackSlomo" => TrackKind::Slomo { curve: curve_f(pkg, &p, "FloatTrack") },
            "InterpTrackFloatProp" => TrackKind::FloatProp { property: name(&p, "PropertyName"), curve: curve_f(pkg, &p, "FloatTrack") },
            "InterpTrackEvent" => TrackKind::Event {
                keys: structs(pkg, &p, "EventTrack").iter().map(|k| (f(k, "Time", 0.0), name(k, "EventName"))).filter(|k| !k.1.is_empty()).collect(),
                forwards: b(&p, "bFireEventsWhenForwards", true),
                backwards: b(&p, "bFireEventsWhenBackwards", true),
            },
            "InterpTrackAkEvent" => TrackKind::AkEvent {
                keys: structs(pkg, &p, "AkEvents")
                    .iter()
                    .filter_map(|k| {
                        let ev = find(k, "Event").and_then(|x| x.as_object()).filter(|o| *o != 0)?;
                        let kp = obj_props(pkg, k, "Properties");
                        Some((f(k, "Time", 0.0), pkg.obj_name(ObjRef::from_index(ev)), b(&kp, "m_bStopAkEvent", false)))
                    })
                    .collect(),
            },
            "InterpTrackToggle" => TrackKind::Toggle {
                keys: structs(pkg, &p, "ToggleTrack").iter().map(|k| (f(k, "Time", 0.0), enum_idx(k, "ToggleAction", &["ETTA_Off", "ETTA_On", "ETTA_Toggle", "ETTA_Trigger", "ETTA_Nothing"], 0))).collect(),
            },
            "InterpTrackVisibility" => TrackKind::Visibility {
                keys: structs(pkg, &p, "VisibilityTrack").iter().map(|k| (f(k, "Time", 0.0), enum_idx(k, "Action", &["EVTA_Hide", "EVTA_Show", "EVTA_Toggle"], 0))).collect(),
            },
            "InterpTrackDialog" => TrackKind::Dialog {
                keys: structs(pkg, &p, "DialogKeys")
                    .iter()
                    .map(|k| {
                        let kp = obj_props(pkg, k, "Properties");
                        let alts = match find(&kp, "m_AltBlurbsGUID").and_then(|x| x.array_raw()) {
                            Some((n, raw)) if raw.len() >= n * 16 => (0..n).map(|i| raw[i * 16..i * 16 + 16].chunks(4).map(|c| format!("{:08X}", u32::from_le_bytes([c[0], c[1], c[2], c[3]]))).collect::<String>()).collect(),
                            _ => Vec::new(),
                        };
                        DialogKey { start: f(k, "StartTime", 0.0), length: f(k, "KeyLength", 0.0), blurb: guid(&kp, "m_BlurbGUID"), alts, wait: b(&kp, "WaitForEndOfBlurb", false) }
                    })
                    .collect(),
            },
            "InterpTrackLookAt" => TrackKind::LookAt { keys: span_keys(pkg, &p, "LookAtKeys"), weight: curve_f(pkg, &p, "FloatTrack") },
            "InterpTrackFaceTo" => TrackKind::FaceTo { keys: span_keys(pkg, &p, "FaceToKeys"), weight: curve_f(pkg, &p, "FloatTrack") },
            "InterpTrackLocomotion" => TrackKind::Locomotion { keys: span_keys(pkg, &p, "LocoKeys") },
            "InterpTrackAttachment" => TrackKind::Attachment {
                target: name(&p, "m_TargetName"),
                keys: structs(pkg, &p, "m_AttachmentKeys")
                    .iter()
                    .map(|k| {
                        let kp = obj_props(pkg, k, "Properties");
                        let info = match find(&kp, "m_AttachmentInfo").map(|x| &x.value) {
                            Some(PV::Struct(_, s)) => s.clone(),
                            _ => Vec::new(),
                        };
                        AttachKey {
                            time: f(k, "Time", 0.0),
                            detach: enum_idx(&kp, "m_KeyType", &["EKAT_Attach", "EKAT_Detach"], 0) == 1,
                            bone: name(&info, "BoneName"),
                            offset: vec(&info, "RelativeOffset"),
                            rotation: find(&info, "RelativeRotation").and_then(|x| x.as_rotator()).unwrap_or([0; 3]),
                            use_offset: b(&info, "bUseRelativeOffset", false),
                            use_rotation: b(&info, "bUseRelativeRotation", false),
                            hard: b(&info, "bHardAttach", false),
                        }
                    })
                    .collect(),
                detach_on_interrupt: b(&p, "m_bDetachActorOnInteruption", false),
            },
            "DisInterpTrackAllowPlayerLook" => TrackKind::AllowPlayerLook {
                keys: structs(pkg, &p, "m_TrackLookAngleKeys")
                    .iter()
                    .map(|k| {
                        let kp = obj_props(pkg, k, "m_pProperties");
                        (f(k, "m_fKeyStartTime", 0.0), [f(&kp, "m_fYaw_Min", 0.0), f(&kp, "m_fYaw_Max", 0.0), f(&kp, "m_fPitch_Min", 0.0), f(&kp, "m_fPitch_Max", 0.0)])
                    })
                    .collect(),
            },
            "InterpTrackSoireeControl" => TrackKind::SoireeControl {
                keys: structs(pkg, &p, "SoireeControlKeys")
                    .iter()
                    .map(|k| {
                        let kp = obj_props(pkg, k, "Properties");
                        SoireeKey {
                            start: f(k, "StartTime", 0.0),
                            length: f(k, "KeyLength", 0.0),
                            pause: enum_idx(&kp, "m_SoireeControlType", &["ESCT_Loop", "ESCT_Pause"], 0) == 1,
                            break_now: enum_idx(&kp, "m_BreakMode", &["ESBM_BreakAtNextLoop", "ESBM_BreakImmediately"], 0) == 1,
                            loops: match find(&kp, "m_LoopCount").map(|x| &x.value) {
                                Some(PV::Int(i)) => *i,
                                _ => 0,
                            },
                            pin: name(&kp, "m_PinName"),
                            blend_out: f(&kp, "m_BreakImmediatelyBlendOut", 0.5),
                        }
                    })
                    .collect(),
            },
            "InterpTrackFaceFX" => TrackKind::FaceFx { keys: structs(pkg, &p, "FaceFXSeqs").iter().map(|k| (f(k, "StartTime", 0.0), name(k, "FaceFXGroupName"), name(k, "FaceFXSeqName"))).collect() },
            _ => TrackKind::Other,
        };
        let default_title = match class.as_str() {
            "InterpTrackMove" => "Movement",
            "InterpTrackStretchAnimControl" => "StrechAnim",
            _ => "Track",
        };
        Track { title: if find(&p, "TrackTitle").is_some() { name(&p, "TrackTitle") } else { default_title.into() }, disabled: b(&p, "bDisableTrack", false), kind, class }
    }

    fn group(pkg: &Package, e: usize) -> Group {
        let class = pkg.export_class(e);
        let p = pkg.read_object(e).map(|h| h.properties).unwrap_or_default();
        let default_name = match class.as_str() {
            "InterpGroupAI" => "AIGroup",
            "InterpGroupPlayer" => "PlayerGroup",
            "InterpGroupDirector" => "DirGroup",
            _ => "InterpGroup",
        };
        let kind = match class.as_str() {
            "InterpGroupDirector" => GroupKind::Director,
            "InterpGroupAI" => GroupKind::Ai {
                stage_mark: name(&p, "StageMarkGroup"),
                snap_to_root: b(&p, "SnapToRootBoneLocationWhenFinished", false),
                lock_requests: b(&p, "LockRequests", true),
                can_interrupt: b(&p, "CanInterruptMatinee", true),
                detached: b(&p, "bShouldBeDetached", false),
            },
            "InterpGroupPlayer" => GroupKind::Player {
                stage_mark: name(&p, "m_StageMarkGroup"),
                blend_time: f(&p, "m_fTimeToBlendToStageMark", 0.2),
                snap_location: b(&p, "m_bSnapToRootBoneLocationWhenFinished", false),
                snap_rotation: b(&p, "m_bSnapToRootBoneRotationWhenFinished", false),
                disable_collision: b(&p, "m_bDisablePlayerCollision", true),
                camera_to_dialog: b(&p, "m_bLeaveCameraControlToDialogTree", false),
            },
            _ if b(&p, "bIsFolder", false) => GroupKind::Folder,
            _ => GroupKind::Plain,
        };
        let tracks = find(&p, "InterpTracks").and_then(|x| x.array_objects().ok()).unwrap_or_default();
        let anim_sets = find(&p, "GroupAnimSets")
            .and_then(|x| x.array_objects().ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|o| *o != 0)
            .map(|o| {
                let r = ObjRef::from_index(o);
                let path = pkg.obj_path(r);
                match r {
                    // An import lives in the package its path starts with.
                    ObjRef::Import(_) => (path.split('.').next().unwrap_or_default().to_string(), path),
                    _ => (pkg.name.clone(), path),
                }
            })
            .collect();
        Group {
            name: if find(&p, "GroupName").is_some() { name(&p, "GroupName") } else { default_name.into() },
            kind,
            parented: b(&p, "bIsParented", false),
            anim_sets,
            tracks: tracks.into_iter().filter_map(|t| match ObjRef::from_index(t) {
                ObjRef::Export(te) => Some(track(pkg, te)),
                _ => None,
            }).collect(),
        }
    }

    /// Read a matinee from an `InterpData` export (following `m_Data` to its `MatineeData`)
    /// or a `MatineeData` export.
    pub fn load(pkg: &Package, export: usize) -> Option<Matinee> {
        let mut e = export;
        let mut p = pkg.read_object(e).ok()?.properties;
        if pkg.export_class(e) == "InterpData" {
            if let Some(ObjRef::Export(m)) = find(&p, "m_Data").and_then(|x| x.as_object()).map(ObjRef::from_index) {
                e = m;
                p = pkg.read_object(e).ok()?.properties;
            }
        }
        let run: Vec<Property> = match find(&p, "m_RunData").map(|x| &x.value) {
            Some(PV::Struct(_, r)) => r.clone(),
            _ => p.clone(),
        };
        let groups = find(&run, "InterpGroups").and_then(|x| x.array_objects().ok()).unwrap_or_default();
        let conv = guid(&run, "m_ConversationNodeGUID");
        Some(Matinee {
            package: pkg.name.clone(),
            export: e as u32,
            path: pkg.obj_path(ObjRef::Export(e)),
            length: f(&run, "InterpLength", 5.0),
            groups: groups.into_iter().filter_map(|g| match ObjRef::from_index(g) {
                ObjRef::Export(ge) => Some(group(pkg, ge)),
                _ => None,
            }).collect(),
            conversation_node: (!conv.is_empty()).then_some(conv),
        })
    }

    /// Soiree markers and length of an AnimSequence export.
    pub fn seq_info(pkg: &Package, export: usize) -> Option<SeqInfo> {
        let h = pkg.read_object(export).ok()?;
        let mut info = SeqInfo { length: f(&h.properties, "SequenceLength", 0.0), markers: Vec::new() };
        for ev in structs(pkg, &h.properties, "Notifies") {
            let Some(ObjRef::Export(ni)) = find(&ev, "Notify").and_then(|x| x.as_object()).map(ObjRef::from_index) else { continue };
            let m = soiree_marker(&pkg.export_class(ni));
            if m < 6 {
                info.markers.push((f(&ev, "Time", 0.0), m));
            }
        }
        Some(info)
    }
}

#[cfg(feature = "load")]
pub use self::load_impl::seq_info;

// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(t: f32, v: f32, mode: CurveMode) -> CurvePoint<f32> {
        CurvePoint { t, v, arrive: 0.0, leave: 0.0, mode }
    }

    #[test]
    fn curve_modes() {
        let c = Curve { points: vec![pt(1.0, 0.0, CurveMode::Linear), pt(3.0, 10.0, CurveMode::Constant), pt(5.0, 20.0, CurveMode::CurveUser), pt(7.0, 0.0, CurveMode::Linear)] };
        assert_eq!(c.eval(0.0, 9.0), 0.0); // before the first key: its value
        assert_eq!(c.eval(2.0, 9.0), 5.0); // linear
        assert_eq!(c.eval(4.0, 9.0), 10.0); // constant holds
        // Hermite with flat tangents: the smoothstep between 20 and 0.
        assert!((c.eval(6.0, 9.0) - 10.0).abs() < 1e-5);
        assert!((c.eval(5.5, 9.0) - 20.0 * (1.0 - (3.0 * 0.0625 - 2.0 * 0.015625))).abs() < 1e-4);
        assert_eq!(c.eval(8.0, 9.0), 0.0); // after the last key
        assert_eq!(CurveF::default().eval(1.0, 9.0), 9.0);
    }

    #[test]
    fn hermite_uses_tangents_scaled_by_gap() {
        // P0=0, P1=0 with leave tangent 1/s over a 2 s gap: value at the middle = T0 * 2 / 8.
        let c = Curve { points: vec![CurvePoint { t: 0.0, v: 0.0, arrive: 0.0, leave: 1.0, mode: CurveMode::CurveUser }, CurvePoint { t: 2.0, v: 0.0, arrive: 0.0, leave: 0.0, mode: CurveMode::CurveUser }] };
        assert!((c.eval(1.0, 0.0) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn move_relative_to_initial() {
        let tr = TrackKind::Move {
            pos: Curve { points: vec![CurvePoint { t: 0.0, v: [0.0; 3], arrive: [0.0; 3], leave: [0.0; 3], mode: CurveMode::Linear }, CurvePoint { t: 1.0, v: [100.0, 0.0, 0.0], arrive: [0.0; 3], leave: [0.0; 3], mode: CurveMode::Linear }] },
            euler: Curve { points: vec![CurvePoint { t: 0.0, v: [0.0, 0.0, 90.0], arrive: [0.0; 3], leave: [0.0; 3], mode: CurveMode::Linear }] },
            frame: MoveFrame::RelativeToInitial,
            rot_mode: 0,
            look_at: String::new(),
        };
        // Initial yaw 90 degrees at (10, 0, 0): relative +X becomes world +Y.
        let init = Xform::new([10.0, 0.0, 0.0], [0, 16384, 0]);
        let x = tr.move_world(1.0, &init).unwrap();
        assert!((x.pos - Vec3::new(10.0, 100.0, 0.0)).length() < 1e-3, "{:?}", x.pos);
        assert!((x.yaw_deg() - 180.0).abs() < 1e-3 || (x.yaw_deg() + 180.0).abs() < 1e-3);
        let mut w = tr.clone();
        if let TrackKind::Move { frame, .. } = &mut w {
            *frame = MoveFrame::World;
        }
        assert!((w.move_world(0.5, &init).unwrap().pos - Vec3::new(50.0, 0.0, 0.0)).length() < 1e-3);
    }

    #[test]
    fn rotator_roundtrip() {
        let m = rot_matrix_deg(20.0, -70.0, 10.0);
        let r = matrix_rotator_deg(&m);
        assert!((r[0] - 20.0).abs() < 1e-3 && (r[1] + 70.0).abs() < 1e-3 && (r[2] - 10.0).abs() < 1e-3, "{r:?}");
    }

    fn key(start: f32, name: &str) -> AnimKey {
        AnimKey { start, name: name.into(), rate: 1.0, ..Default::default() }
    }

    #[test]
    fn anim_keys() {
        let seq = |n: &str| Some(SeqInfo { length: if n == "A" { 2.0 } else { 4.0 }, markers: vec![] });
        let mut keys = vec![key(1.0, "A"), key(5.0, "B")];
        keys[1].looping = true;
        keys[1].start_offset = 1.0;
        keys[1].end_offset = 1.0;
        let s = anim_at(&keys, 0.5, &seq).unwrap();
        assert_eq!((s.key, s.position), (0, 0.0)); // before the first key
        let s = anim_at(&keys, 2.0, &seq).unwrap();
        assert_eq!((s.key, s.position, s.finished), (0, 1.0, false));
        let s = anim_at(&keys, 4.0, &seq).unwrap();
        assert!(s.finished && (s.position - 2.0).abs() < 1e-3); // clamped to the end
        // Looping key: trimmed length 2 s from offset 1: t=8 -> 3 s in -> 1 + (3 mod 2) = 2.
        let s = anim_at(&keys, 8.0, &seq).unwrap();
        assert_eq!((s.key, s.position), (1, 2.0));
        let mut r = keys.clone();
        r[0].reverse = true;
        assert!((anim_at(&r, 1.5, &seq).unwrap().position - 1.5).abs() < 1e-5);
    }

    #[test]
    fn stretched_key_loops_the_end_segment() {
        // Sequence 3 s: Loop marker at 1 s, End marker at 2 s. The key extends End by 4 s
        // (end offset -4): the 1..2 s segment fills 5 s, i.e. loops 5 times.
        let seq = |_: &str| Some(SeqInfo { length: 3.0, markers: vec![(1.0, 2), (2.0, 3)] });
        let mut k = key(0.0, "S");
        k.end_offset = -4.0;
        k.soiree[3] = 4.0;
        let keys = vec![k];
        let at = |t: f32| stretch_at(&keys, t, &seq).unwrap();
        assert!((at(0.5).position - 0.5).abs() < 1e-4); // In..Loop: 1:1
        assert!((at(1.5).position - 1.5).abs() < 1e-4);
        assert!((at(2.25).position - 1.25).abs() < 1e-4 && at(2.25).looping); // second loop
        assert!((at(5.75).position - 1.75).abs() < 1e-4);
        assert!((at(6.5).position - 2.5).abs() < 1e-4); // tail after End: 1:1
        assert!(at(7.5).finished);
        // A shifted Out marker stretches linearly instead.
        let seq2 = |_: &str| Some(SeqInfo { length: 4.0, markers: vec![(2.0, 5)] });
        let mut k2 = key(0.0, "S");
        k2.soiree[5] = 2.0; // Out at 4 s instead of 2 s
        k2.end_offset = -2.0;
        let keys2 = vec![k2];
        assert!((stretch_at(&keys2, 2.0, &seq2).unwrap().position - 1.0).abs() < 1e-4);
        assert!((stretch_at(&keys2, 5.0, &seq2).unwrap().position - 3.0).abs() < 1e-4);
    }

    #[test]
    fn director_cuts_and_fade() {
        let m = Matinee {
            length: 10.0,
            groups: vec![Group {
                name: "DirGroup".into(),
                kind: GroupKind::Director,
                tracks: vec![
                    Track { kind: TrackKind::Director { cuts: vec![DirectorCut { time: 1.0, transition: 0.0, group: "CamA".into() }, DirectorCut { time: 4.0, transition: 0.5, group: "CamB".into() }] }, ..Default::default() },
                    Track { kind: TrackKind::Fade { curve: Curve { points: vec![pt(0.0, 1.0, CurveMode::Linear), pt(2.0, 0.0, CurveMode::Linear)] }, persist: false }, ..Default::default() },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(m.director_cut(0.5).is_none());
        assert_eq!(m.director_cut(1.0).unwrap().0.group, "CamA");
        assert_eq!(m.director_cut(3.9).unwrap().1, 0);
        assert_eq!(m.director_cut(9.0).unwrap().0.group, "CamB");
        assert_eq!(m.fade(1.0), Some(0.5));
        assert!(crossed(1.0, 0.5, 1.0) && !crossed(1.0, 1.0, 1.5) && crossed(1.0, 1.5, 1.0));
    }

    fn soiree(keys: Vec<SoireeKey>) -> Matinee {
        Matinee { length: 10.0, groups: vec![Group { tracks: vec![Track { kind: TrackKind::SoireeControl { keys }, ..Default::default() }], ..Default::default() }], ..Default::default() }
    }

    #[test]
    fn soiree_loop_until_broken() {
        let m = soiree(vec![SoireeKey { start: 0.0, length: 1.0, pin: "BlackLoop".into(), break_now: true, ..Default::default() }]);
        let mut s = PlayState::default();
        let mut pos = 0.0;
        for _ in 0..100 {
            let st = s.step(&m, pos, pos + 0.1);
            pos = st.pos;
        }
        assert!(pos <= 1.0 + 1e-4, "held in the loop: {pos}");
        s.break_pin("blackloop");
        let st = s.step(&m, pos, pos + 0.1);
        assert!(st.jumped && st.pos > 1.0, "broken immediately: {st:?}");
        let st2 = s.step(&m, st.pos, st.pos + 0.1);
        assert!(!st2.jumped && (st2.pos - st.pos - 0.1).abs() < 1e-5);
    }

    #[test]
    fn soiree_loop_count_and_pause() {
        let m = soiree(vec![SoireeKey { start: 1.0, length: 1.0, loops: 2, ..Default::default() }, SoireeKey { start: 5.0, pause: true, pin: "Go".into(), ..Default::default() }]);
        let mut s = PlayState::default();
        let mut pos = 0.0f32;
        let mut wraps = 0;
        for _ in 0..80 {
            let st = s.step(&m, pos, pos + 0.1);
            wraps += st.jumped as i32;
            pos = st.pos;
        }
        // Loop count 2: one wrap back, then through; then held at the pause key (5 s).
        assert_eq!(wraps, 1);
        assert!((pos - 5.0).abs() < 1e-4, "{pos}");
        s.break_pin("Go");
        assert!(s.step(&m, pos, pos + 0.1).pos > 5.0);
    }

    struct W;
    impl SceneWorld for W {
        fn xform(&self, b: Bound) -> Option<Xform> {
            Some(match b {
                Bound::Actor(0) => Xform::new([1000.0, 0.0, 0.0], [0, 16384, 0]), // boat, yaw 90
                Bound::Actor(_) => Xform::new([0.0, 500.0, 0.0], [0, 0, 0]),      // stage mark
                Bound::Pawn(_) => Xform::new([-50.0, -50.0, 0.0], [0, 0, 0]),
            })
        }
        fn bone(&self, _actor: u32, bone: &str) -> Option<Xform> {
            (bone == "Seat").then(|| Xform::new([0.0, 10.0, 20.0], [0, 0, 0]))
        }
    }

    #[test]
    fn scene_moves_attaches_and_detaches() {
        let pt3 = |t: f32, v: [f32; 3]| CurvePoint { t, v, arrive: [0.0; 3], leave: [0.0; 3], mode: CurveMode::Linear };
        let boat = Group {
            name: "Boat".into(),
            tracks: vec![
                Track { kind: TrackKind::Move { pos: Curve { points: vec![pt3(0.0, [0.0; 3]), pt3(10.0, [100.0, 0.0, 0.0])] }, euler: Curve::default(), frame: MoveFrame::RelativeToInitial, rot_mode: 0, look_at: String::new() }, ..Default::default() },
                Track {
                    kind: TrackKind::Attachment {
                        target: "PlayerGroup".into(),
                        keys: vec![AttachKey { time: 1.0, bone: "Seat".into(), offset: [5.0, 0.0, 0.0], use_offset: true, ..Default::default() }, AttachKey { time: 6.0, detach: true, ..Default::default() }],
                        detach_on_interrupt: false,
                    },
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let player = Group { name: "PlayerGroup".into(), kind: GroupKind::Player { stage_mark: "Mark".into(), blend_time: 0.0, snap_location: false, snap_rotation: false, disable_collision: true, camera_to_dialog: false }, ..Default::default() };
        let mark = Group { name: "Mark".into(), ..Default::default() };
        let m = Matinee { length: 10.0, groups: vec![boat, player, mark], ..Default::default() };
        let scene = Scene::new(&m, vec![vec![Bound::Actor(0)], vec![Bound::Pawn(7)], vec![Bound::Actor(1)]], &W);
        let pose = |t: f32| scene.update(&m, t - 0.1, t, &W).poses.iter().find(|p| p.0 == Bound::Pawn(7)).map(|p| p.1.pos);
        // Before the attach key: at the stage mark.
        assert!((pose(0.5).unwrap() - Vec3::new(0.0, 500.0, 0.0)).length() < 1e-3);
        // Attached: boat (relative +X turned by yaw 90 => +Y) then bone (0,10,20) then offset (5,0,0), all yawed.
        // Boat at t=5: (1000, 50, 0); bone in boat space (0,10,20) -> world (-10, 0, 20); offset -> (0, 5, 0).
        let p5 = pose(5.0).unwrap();
        assert!((p5 - Vec3::new(990.0, 55.0, 20.0)).length() < 1e-2, "{p5:?}");
        // Detached at 6 s: stays where the boat left it.
        let p6 = pose(6.0 - 1e-4).unwrap();
        let p9 = pose(9.0).unwrap();
        assert!((p9 - p6).length() < 1e-2, "{p6:?} {p9:?}");
        // The boat keeps moving.
        let boat9 = scene.update(&m, 8.9, 9.0, &W).poses.iter().find(|p| p.0 == Bound::Actor(0)).unwrap().1.pos;
        assert!((boat9 - Vec3::new(1000.0, 90.0, 0.0)).length() < 1e-2);
        assert_eq!(scene.controlled_pawns(&m), vec![7]);
    }

    #[test]
    fn dialog_wait_holds_until_the_blurb_ends() {
        let m = Matinee { length: 10.0, groups: vec![Group { tracks: vec![Track { kind: TrackKind::Dialog { keys: vec![DialogKey { start: 1.0, length: 2.0, blurb: "AB".into(), alts: vec![], wait: true }] }, ..Default::default() }], ..Default::default() }], ..Default::default() };
        let mut s = PlayState { speaking: vec!["AB".into()], ..Default::default() };
        let st = s.step(&m, 2.95, 3.05);
        assert!(st.pos < 3.0);
        s.speaking.clear();
        assert!((s.step(&m, st.pos, st.pos + 0.1).pos - st.pos - 0.1).abs() < 1e-6);
    }
}
