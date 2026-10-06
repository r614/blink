//! FaceFX archives cooked into `FaceFXAsset` (`RawFaceFXActorData`) and `FaceFXAnimSet`
//! (`RawFaceFXAnimSetData`), and their evaluation. Layout decoded from the retail data
//! (research/dishonored/ANIMATION.md §12).
//!
//! Every archive: `FACE`, version 1731, licensee and project strings, a class directory and a
//! name table; objects then refer to names by index.
//! - Actor (`FaceFXAsset`, e.g. `Npc_Servants.Mesh.Skm_ServantA_Head_FaceFX`): the master bone
//!   list (every skeleton bone with its reference transform and the bone pose nodes that move
//!   it) and the compiled face graph (nodes in evaluation order, each with its inputs through
//!   link functions).
//! - Anim set (`FaceFXAnimSet`, one per `DisDialogVoiceData`, `m_pFaceFXAnimSet`): one group of
//!   animations named by the blurb GUID, each a set of Hermite curves named after face graph
//!   nodes, with blend-in / blend-out times.

use anyhow::{Context, Result, bail, ensure};
use ue3_package::Package;

/// Little-endian cursor over an archive.
pub struct Cur<'a> {
    pub d: &'a [u8],
    pub pos: usize,
}

impl<'a> Cur<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Self { d, pos: 0 }
    }
    pub fn u16(&mut self) -> Result<u16> {
        ensure!(self.pos + 2 <= self.d.len(), "facefx: read past the end at {:#x}", self.pos);
        let v = u16::from_le_bytes([self.d[self.pos], self.d[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }
    pub fn u32(&mut self) -> Result<u32> {
        ensure!(self.pos + 4 <= self.d.len(), "facefx: read past the end at {:#x}", self.pos);
        let v = u32::from_le_bytes(self.d[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    /// Count-prefixed string (the header strings count their terminator).
    pub fn string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        ensure!(n < 1 << 16 && self.pos + n <= self.d.len(), "facefx: bad string length {n} at {:#x}", self.pos - 4);
        let s = &self.d[self.pos..self.pos + n];
        self.pos += n;
        let s = s.strip_suffix(&[0]).unwrap_or(s);
        Ok(String::from_utf8_lossy(s).into_owned())
    }
    pub fn count(&mut self, max: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        ensure!(n <= max, "facefx: count {n} too large at {:#x}", self.pos - 4);
        Ok(n)
    }
}

/// Archive header: version, the class directory and the name table.
#[derive(Debug, Clone)]
pub struct Header {
    pub version: u32,
    /// (class name, class version)
    pub classes: Vec<(String, u16)>,
    pub names: Vec<String>,
}

pub fn read_header(c: &mut Cur) -> Result<Header> {
    let magic = c.u32()?;
    ensure!(magic == u32::from_le_bytes(*b"FACE"), "facefx: no FACE magic");
    let version = c.u32()?;
    let _ = c.u32()?;
    let _licensee = c.string()?;
    let _project = c.string()?;
    let _licensee_version = c.u32()?;
    let _endian = c.u16()?;
    let nclasses = c.count(256)?;
    let mut classes = Vec::with_capacity(nclasses);
    for _ in 0..nclasses {
        let _ = c.u32()?;
        let _ = c.u32()?;
        let name = c.string()?;
        let ver = c.u16()?;
        classes.push((name, ver));
    }
    let nnames = c.count(1 << 16)?;
    let mut names = Vec::with_capacity(nnames);
    for _ in 0..nnames {
        names.push(c.string()?);
    }
    Ok(Header { version, classes, names })
}

/// The archive bytes of a `FaceFXAsset` / `FaceFXAnimSet` export: after the (empty) tagged
/// properties comes the native byte array.
pub fn raw_archive(pkg: &Package, export: usize) -> Result<&[u8]> {
    let d = pkg.export_data(export)?;
    let h = pkg.read_object(export).context("facefx object properties")?;
    let mut off = h.native_offset;
    ensure!(off + 4 <= d.len(), "facefx: no archive");
    let n = u32::from_le_bytes(d[off..off + 4].try_into().unwrap()) as usize;
    off += 4;
    ensure!(off + n <= d.len(), "facefx: archive length {n} past the export");
    let a = &d[off..off + n];
    if &a[..4.min(a.len())] != b"FACE" {
        bail!("facefx: archive does not start with FACE");
    }
    Ok(a)
}

fn name_at(names: &[String], i: u32) -> Result<String> {
    names.get(i as usize).cloned().with_context(|| format!("facefx: name index {i} out of {}", names.len()))
}

// ---------------------------------------------------------------------------------------------
// Actor

/// A bone transform in UE3 mesh space (the archive's right-handed values with Y mirrored:
/// position y and rotation y negated; rotations stored w, x, y, z, kept here as x, y, z, w,
/// the convention of `SkeletalMesh` bones in the port).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoneXf {
    pub pos: [f32; 3],
    pub rot: [f32; 4],
    pub scale: [f32; 3],
}

fn read_quat(c: &mut Cur) -> Result<[f32; 4]> {
    let w = c.f32()?;
    let (x, y, z) = (c.f32()?, c.f32()?, c.f32()?);
    Ok([x, -y, z, w])
}

fn read_xf(c: &mut Cur) -> Result<BoneXf> {
    let pos = [c.f32()?, -c.f32()?, c.f32()?];
    let rot = read_quat(c)?;
    let scale = [c.f32()?, c.f32()?, c.f32()?];
    Ok(BoneXf { pos, rot, scale })
}

/// One bone pose node's target for a bone: position and scale relative to the reference,
/// rotation absolute (bone-local).
#[derive(Debug, Clone)]
pub struct PoseLink {
    pub node: u32,
    pub xf: BoneXf,
}

/// `FxMasterBoneList` entry: the reference bone and the bone pose nodes that move it.
#[derive(Debug, Clone)]
pub struct MasterBone {
    pub name: String,
    pub reference: BoneXf,
    pub inv_ref_rot: [f32; 4],
    pub index: u32,
    pub weight: f32,
    pub poses: Vec<PoseLink>,
}

/// Link function of a face graph edge (FaceFX's standard set; the compiled type number is the
/// first field).
#[derive(Debug, Clone)]
pub struct Link {
    pub node: u32,
    pub func: u32,
    pub params: Vec<f32>,
}

/// Compiled face graph node.
#[derive(Debug, Clone)]
pub struct Node {
    /// 0 combiner, 4 bone pose (the only two in the retail actors).
    pub kind: u32,
    pub name: String,
    pub min: f32,
    pub max: f32,
    /// Input operation (0 = sum; every retail node).
    pub op: u32,
    pub inputs: Vec<Link>,
}

pub const NODE_COMBINER: u32 = 0;
pub const NODE_BONE_POSE: u32 = 4;

#[derive(Debug, Clone)]
pub struct Actor {
    pub name: String,
    pub bones: Vec<MasterBone>,
    pub nodes: Vec<Node>,
}

pub fn read_actor(raw: &[u8]) -> Result<Actor> {
    let mut c = Cur::new(raw);
    let h = read_header(&mut c)?;
    let n = &h.names;
    let name = name_at(n, c.u32()?)?;
    let _ = c.u32()?;
    let nbones = c.count(4096)?;
    let mut bones = Vec::with_capacity(nbones);
    for _ in 0..nbones {
        let at = c.pos;
        let bname = name_at(n, c.u32()?).with_context(|| format!("bone at {at:#x}"))?;
        let reference = read_xf(&mut c)?;
        let inv_ref_rot = read_quat(&mut c)?;
        let index = c.u32()?;
        let weight = c.f32()?;
        let np = c.count(4096)?;
        let mut poses = Vec::with_capacity(np);
        for _ in 0..np {
            let node = c.u32()?;
            let _bone = c.u32()?;
            let xf = read_xf(&mut c)?;
            poses.push(PoseLink { node, xf });
        }
        bones.push(MasterBone { name: bname, reference, inv_ref_rot, index, weight, poses });
    }
    let nnodes = c.count(1 << 14)?;
    let mut nodes = Vec::with_capacity(nnodes);
    for i in 0..nnodes {
        let at = c.pos;
        let kind = c.u32()?;
        let name = name_at(n, c.u32()?).with_context(|| format!("node at {at:#x}"))?;
        // min, 1/min (1 when min is 0), max, 1/max.
        let (min, _, max, _) = (c.f32()?, c.f32()?, c.f32()?, c.f32()?);
        let op = c.u32()?;
        let ni = c.count(1024)?;
        let mut inputs = Vec::with_capacity(ni);
        for _ in 0..ni {
            let node = c.u32()?;
            ensure!((node as usize) < i, "facefx: node {i} input {node} not evaluated before it");
            let func = c.u32()?;
            let np = c.count(64)?;
            let mut params = Vec::with_capacity(np);
            for _ in 0..np {
                params.push(c.f32()?);
            }
            inputs.push(Link { node, func, params });
        }
        let _ = c.u32()?;
        nodes.push(Node { kind, name, min, max, op, inputs });
    }
    for b in &bones {
        for p in &b.poses {
            ensure!((p.node as usize) < nodes.len(), "facefx: bone {} pose node {} out of range", b.name, p.node);
        }
    }
    // The rest is a name -> node lookup table (hash buckets), not needed here.
    Ok(Actor { name, bones, nodes })
}

/// FaceFX link function `func` with `params` applied to `x`. Types: 1 linear (m, b),
/// 2 quadratic (a), 3 cubic (a), 4 square root (a), 5 negate, 6 inverse, 7 one-clamp,
/// 8 constant (c), 10 clamped linear (x0, y0, x1, y1). The numbering follows FaceFX's
/// standard function list; only 1 and 2 occur in the retail actors.
pub fn link_fn(func: u32, p: &[f32], x: f32) -> f32 {
    let a = |i: usize, d: f32| p.get(i).copied().unwrap_or(d);
    match func {
        1 => a(0, 1.0) * x + a(1, 0.0),
        2 => a(0, 1.0) * x * x,
        3 => a(0, 1.0) * x * x * x,
        4 => a(0, 1.0) * x.max(0.0).sqrt(),
        5 => -x,
        6 => {
            if x.abs() > 1e-6 {
                1.0 / x
            } else {
                0.0
            }
        }
        7 => 1.0 - x.clamp(0.0, 1.0),
        8 => a(0, 0.0),
        10 => {
            let (x0, y0, x1, y1) = (a(0, 0.0), a(1, 0.0), a(2, 1.0), a(3, 1.0));
            if (x1 - x0).abs() < 1e-6 {
                y0
            } else {
                let t = ((x - x0) / (x1 - x0)).clamp(0.0, 1.0);
                y0 + (y1 - y0) * t
            }
        }
        _ => 0.0,
    }
}

impl Actor {
    pub fn node(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.name.eq_ignore_ascii_case(name))
    }

    /// Node values for the given track contributions (by node index): each node is its track
    /// value plus its inputs through their link functions (summed), clamped to its range.
    pub fn evaluate(&self, track: &[f32]) -> Vec<f32> {
        let mut v = vec![0.0f32; self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            let mut x = if n.op == 1 && !n.inputs.is_empty() { 1.0 } else { 0.0 };
            for l in &n.inputs {
                let y = link_fn(l.func, &l.params, v[l.node as usize]);
                if n.op == 1 { x *= y } else { x += y }
            }
            x += track.get(i).copied().unwrap_or(0.0);
            v[i] = if n.min <= n.max { x.clamp(n.min, n.max) } else { x };
        }
        v
    }

    /// Bone-local offsets from the reference for the node values: per moved bone, the
    /// position delta and the rotation delta (reference^-1 * blended rotation, x y z w).
    /// Positions add their weighted deltas; rotations compose each pose's delta from the
    /// reference, scaled by its weight (slerp from identity).
    pub fn bone_deltas(&self, values: &[f32]) -> Vec<(usize, [f32; 3], [f32; 4])> {
        let mut out = Vec::new();
        for (bi, b) in self.bones.iter().enumerate() {
            let mut pos = [0.0f32; 3];
            let mut rot = [0.0f32, 0.0, 0.0, 1.0];
            let mut any = false;
            for p in &b.poses {
                let w = values.get(p.node as usize).copied().unwrap_or(0.0) * b.weight;
                if w.abs() < 1e-5 {
                    continue;
                }
                any = true;
                for k in 0..3 {
                    pos[k] += p.xf.pos[k] * w;
                }
                let d = quat_mul(quat_conj(b.reference.rot), p.xf.rot);
                rot = quat_mul(rot, quat_pow(d, w));
            }
            if any {
                out.push((bi, pos, rot));
            }
        }
        out
    }
}

fn quat_conj(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// `q` scaled to `w` of its rotation (slerp from identity; `w` may exceed 1 or be negative).
fn quat_pow(q: [f32; 4], w: f32) -> [f32; 4] {
    let q = if q[3] < 0.0 { [-q[0], -q[1], -q[2], -q[3]] } else { q };
    let s = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
    if s < 1e-7 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let half = s.atan2(q[3]) * w;
    let k = half.sin() / s;
    [q[0] * k, q[1] * k, q[2] * k, half.cos()]
}

// ---------------------------------------------------------------------------------------------
// Anim set

/// One curve key: FaceFX Hermite key (time s, value, incoming and outgoing slopes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    pub time: f32,
    pub value: f32,
    pub slope_in: f32,
    pub slope_out: f32,
}

#[derive(Debug, Clone)]
pub struct Curve {
    /// Face graph node the curve drives (a target such as `open`, `W`, `Blink`,
    /// `Orientation_Head_Pitch`).
    pub name: String,
    /// Interpolation (0 = Hermite in every retail curve).
    pub interp: u32,
    pub keys: Vec<Key>,
}

impl Curve {
    /// Value at `t`: held before the first and after the last key, cubic Hermite with the
    /// keys' slopes between them.
    pub fn eval(&self, t: f32) -> f32 {
        let k = &self.keys;
        let Some(first) = k.first() else { return 0.0 };
        if t <= first.time {
            return first.value;
        }
        let last = k[k.len() - 1];
        if t >= last.time {
            return last.value;
        }
        let i = k.partition_point(|x| x.time <= t).max(1) - 1;
        let (a, b) = (k[i], k[i + 1]);
        let dt = b.time - a.time;
        if dt <= 1e-6 {
            return b.value;
        }
        let s = (t - a.time) / dt;
        if self.interp == 1 {
            return a.value + (b.value - a.value) * s;
        }
        if self.interp == 2 {
            return a.value;
        }
        let (s2, s3) = (s * s, s * s * s);
        let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
        let h10 = s3 - 2.0 * s2 + s;
        let h01 = -2.0 * s3 + 3.0 * s2;
        let h11 = s3 - s2;
        h00 * a.value + h10 * dt * a.slope_out + h01 * b.value + h11 * dt * b.slope_in
    }
}

#[derive(Debug, Clone)]
pub struct Anim {
    /// The blurb GUID (`DisVoiceTextBlurb.m_GUID` as 32 hex digits).
    pub name: String,
    pub curves: Vec<Curve>,
    pub blend_in: f32,
    pub blend_out: f32,
    /// The source WAV the curves were generated from.
    pub audio_path: String,
}

impl Anim {
    /// First and last key time over all curves.
    pub fn range(&self) -> (f32, f32) {
        let mut r = (f32::MAX, f32::MIN);
        for c in &self.curves {
            if let (Some(a), Some(b)) = (c.keys.first(), c.keys.last()) {
                r.0 = r.0.min(a.time);
                r.1 = r.1.max(b.time);
            }
        }
        if r.0 > r.1 { (0.0, 0.0) } else { r }
    }

    /// Blend weight at `t`: ramps up over `blend_in` from the start and down over `blend_out`
    /// to the end.
    pub fn weight(&self, t: f32) -> f32 {
        let (s, e) = self.range();
        if t < s || t > e {
            return 0.0;
        }
        let a = if self.blend_in > 1e-4 { ((t - s) / self.blend_in).min(1.0) } else { 1.0 };
        let b = if self.blend_out > 1e-4 { ((e - t) / self.blend_out).min(1.0) } else { 1.0 };
        a.min(b).max(0.0)
    }

    /// Track contributions per actor node at time `t` (curves without a node are ignored).
    pub fn tracks(&self, actor: &Actor, t: f32) -> Vec<f32> {
        let w = self.weight(t);
        let mut v = vec![0.0; actor.nodes.len()];
        if w <= 0.0 {
            return v;
        }
        for c in &self.curves {
            if let Some(i) = actor.node(&c.name) {
                v[i] += c.eval(t) * w;
            }
        }
        v
    }
}

#[derive(Debug, Clone)]
pub struct AnimSet {
    pub name: String,
    /// The actor the set was authored against (e.g. `Skm_ServantA_Head_FaceFX`).
    pub actor: String,
    pub group: String,
    pub anims: Vec<Anim>,
}

pub fn read_anim_set(raw: &[u8]) -> Result<AnimSet> {
    let mut c = Cur::new(raw);
    let h = read_header(&mut c)?;
    let n = &h.names;
    let name = name_at(n, c.u32()?)?;
    let _ = c.u32()?;
    let actor = name_at(n, c.u32()?)?;
    let group = name_at(n, c.u32()?)?;
    let nanims = c.count(4096)?;
    let mut anims = Vec::with_capacity(nanims);
    for _ in 0..nanims {
        let at = c.pos;
        anims.push(read_anim(&mut c, n).with_context(|| format!("anim at {at:#x}"))?);
    }
    ensure!(c.pos == raw.len(), "facefx: anim set parsed to {:#x} of {:#x}", c.pos, raw.len());
    Ok(AnimSet { name, actor, group, anims })
}

fn read_anim(c: &mut Cur, n: &[String]) -> Result<Anim> {
    let name = name_at(n, c.u32()?)?;
    let ncurves = c.count(1024)?;
    let mut curves = Vec::with_capacity(ncurves);
    for _ in 0..ncurves {
        let name = name_at(n, c.u32()?)?;
        let interp = c.u32()?;
        curves.push(Curve { name, interp, keys: Vec::new() });
    }
    // All keys of the animation, then the key count of each curve.
    let nkeys = c.count(1 << 20)?;
    let mut keys = Vec::with_capacity(nkeys);
    for _ in 0..nkeys {
        keys.push(Key { time: c.f32()?, value: c.f32()?, slope_in: c.f32()?, slope_out: c.f32()? });
    }
    let ncounts = c.count(1024)?;
    ensure!(ncounts == ncurves, "facefx: {ncounts} key counts for {ncurves} curves");
    let mut k = 0usize;
    for cv in curves.iter_mut() {
        let m = c.count(1 << 20)?;
        ensure!(k + m <= keys.len(), "facefx: key counts exceed {} keys", keys.len());
        cv.keys = keys[k..k + m].to_vec();
        k += m;
    }
    ensure!(k == keys.len(), "facefx: key counts sum {k} != {}", keys.len());
    let blend_in = c.f32()?;
    let blend_out = c.f32()?;
    // Two zero words (every retail anim), the source WAV, then -1.
    let _ = (c.u32()?, c.u32()?);
    let audio_path = c.string()?;
    let _ = c.i32()?;
    Ok(Anim { name, curves, blend_in, blend_out, audio_path })
}

/// Every `FaceFXAnimSet` / `FaceFXAsset` export of a package, decoded.
pub fn package_anim_sets(pkg: &Package) -> Vec<Result<AnimSet>> {
    (0..pkg.exports.len())
        .filter(|&i| pkg.export_class(i) == "FaceFXAnimSet")
        .map(|i| raw_archive(pkg, i).and_then(read_anim_set))
        .collect()
}

/// The `FaceFXAsset` a `SkeletalMesh` export names (`FaceFXAsset` property), decoded.
pub fn mesh_actor(pkg: &Package, mesh_export: usize) -> Result<Option<Actor>> {
    use ue3_package::ObjRef;
    use ue3_package::properties::{Value, find};
    let h = pkg.read_object(mesh_export)?;
    let Some(p) = find(&h.properties, "FaceFXAsset") else { return Ok(None) };
    let Value::Object(r) = p.value else { return Ok(None) };
    let ObjRef::Export(e) = ObjRef::from_index(r) else { return Ok(None) };
    Ok(Some(read_actor(raw_archive(pkg, e)?)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Content;

    fn content() -> Option<Content> {
        match Content::locate() {
            Ok(c) => Some(c),
            Err(_) => {
                eprintln!("no retail install; skipped");
                None
            }
        }
    }

    #[test]
    fn hermite_and_links() {
        let c = Curve { name: "open".into(), interp: 0, keys: vec![Key { time: 0.0, value: 0.0, slope_in: 0.0, slope_out: 0.0 }, Key { time: 1.0, value: 1.0, slope_in: 0.0, slope_out: 0.0 }] };
        assert_eq!(c.eval(-1.0), 0.0);
        assert!((c.eval(0.5) - 0.5).abs() < 1e-6);
        assert_eq!(c.eval(2.0), 1.0);
        assert!((link_fn(1, &[0.1, 0.0], 5.0) - 0.5).abs() < 1e-6);
        assert!((link_fn(2, &[-0.3], 0.5) + 0.075).abs() < 1e-6);
        let q = quat_pow([0.0, 0.0, (0.5f32).sin(), (0.5f32).cos()], 0.5);
        assert!((q[2] - (0.25f32).sin()).abs() < 1e-5);
    }

    /// Every Boyle FaceFX actor and anim set decodes completely; the servant's face graph and
    /// a servant line are wired as the archive says.
    #[test]
    fn boyle_facefx() {
        let Some(c) = content() else { return };
        let mut sets = 0;
        let mut anims = 0;
        let mut actors = 0;
        for pkg_name in ["L_Boyle_Int_Script", "L_BoyleStreet_Ext_Script", "L_Boyle_Ext_Script"] {
            let pkg = c.package(pkg_name).unwrap();
            for s in package_anim_sets(&pkg) {
                let s = s.unwrap();
                sets += 1;
                anims += s.anims.len();
                for a in &s.anims {
                    assert_eq!(a.name.len(), 32, "{}", a.name);
                    assert!(a.curves.iter().all(|c| c.interp == 0));
                }
            }
            for i in 0..pkg.exports.len() {
                if pkg.export_class(i) == "FaceFXAsset" {
                    let a = read_actor(raw_archive(&pkg, i).unwrap()).unwrap();
                    assert!(a.nodes.iter().all(|n| n.kind == NODE_COMBINER || n.kind == NODE_BONE_POSE), "{}", a.name);
                    assert!(a.nodes.iter().flat_map(|n| &n.inputs).all(|l| l.func == 1 || l.func == 2), "{}", a.name);
                    actors += 1;
                }
            }
        }
        assert!(sets >= 30 && anims >= 300 && actors >= 10, "{sets} sets {anims} anims {actors} actors");

        let pkg = c.package("L_Boyle_Int_Script").unwrap();
        let ai = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ue3_package::ObjRef::Export(i)).ends_with("Skm_ServantA_Head_FaceFX")).unwrap();
        let actor = read_actor(raw_archive(&pkg, ai).unwrap()).unwrap();
        assert_eq!(actor.bones.len(), 109);
        assert_eq!(actor.nodes.len(), 99);
        let open = actor.node("open").unwrap();
        assert_eq!(actor.nodes[open].kind, NODE_BONE_POSE);
        let jaw = actor.bones.iter().find(|b| b.name == "jaw_jnt").unwrap();
        assert!(jaw.poses.iter().any(|p| p.node as usize == open));
        // Head pose: the reference matches the head mesh's bind pose (Y mirrored).
        let head = actor.bones.iter().find(|b| b.name == "head_jnt").unwrap();
        assert!((head.reference.rot[1] - 0.133).abs() < 0.01 && (head.reference.pos[0] - 8.77).abs() < 0.01);
        // Fully open: the jaw turns about its local Y by about 20 degrees.
        let mut track = vec![0.0; actor.nodes.len()];
        track[open] = 1.0;
        let v = actor.evaluate(&track);
        let d = actor.bone_deltas(&v);
        let (_, _, r) = d.iter().find(|x| actor.bones[x.0].name == "jaw_jnt").unwrap();
        let ang = 2.0 * r[3].abs().min(1.0).acos().to_degrees();
        assert!((ang - 20.0).abs() < 2.0, "{ang}");
        // Graph: Surprise drives W through a 0.377 linear link.
        track = vec![0.0; actor.nodes.len()];
        track[actor.node("Surprise").unwrap()] = 1.0;
        let v = actor.evaluate(&track);
        assert!((v[actor.node("W").unwrap()] - 0.3773).abs() < 1e-3);

        let set = package_anim_sets(&pkg).into_iter().map(|s| s.unwrap()).find(|s| s.name == "2A59DC744C690F37ED63A88D3298397F").unwrap();
        assert_eq!(set.actor, "Skm_ServantA_Head_FaceFX");
        assert_eq!(set.anims.len(), 18);
        let a = set.anims.iter().find(|a| a.name == "C9D725BC029083900353E608F7C0F29D").unwrap();
        assert_eq!(a.curves.len(), 19);
        assert!((a.blend_in - 0.16).abs() < 1e-4 && (a.blend_out - 0.22).abs() < 1e-4);
        let (s, e) = a.range();
        assert!(s < 0.0 && (e - 1.08).abs() < 0.01, "{s} {e}");
        // The mouth opens during the line.
        let peak = (0..100).map(|k| a.tracks(&actor, k as f32 * 0.011)[open]).fold(0.0f32, f32::max);
        assert!(peak > 0.2, "{peak}");
    }
}
