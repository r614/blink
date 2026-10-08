//! Per-tick skeletal animation of the original: bone transforms and AnimTree state.
//!
//! A `SkeletalMeshComponent` keeps its pose in `SpaceBases` (+0x208, `array<BoneAtom>`, 32 bytes
//! each: rotation quaternion X Y Z W, translation, scale; component space, written by the
//! component's animation update) and its placement in `LocalToWorld` (+0x60, a row-major
//! matrix with the translation in the last row). The AnimTree (`Animations`, +0x1E0) lists every
//! node in `AnimTickArray` (+0x1A8). Per tick the tick script reads, for each recorded mesh:
//! `LocalToWorld`, the whole `SpaceBases` (in 4096-byte pieces), and per node its flags and
//! `NodeTotalWeight` (+0x5C..+0x74), for sequence nodes `AnimSeqName` / `Rate` / flags /
//! `CurrentTime` (+0xCC..+0xE0), for blend nodes the `Children` array (each child's `Weight`).
//!
//! Output (compact JSON): bone names (`GetBoneName`), node names / classes, and per frame only
//! what changed: `l2w` (16 floats), `cs` / `ws` (bone index -> [qx, qy, qz, qw, tx, ty, tz],
//! quaternions x 1e4 and translations x 100 as integers; `ws` = the bone through `LocalToWorld`,
//! only with `+world` in the bone filter / `world=true`),
//! `nodes` (node index -> [weight x 1e3, relevant] or for a sequence [weight x 1e3, relevant,
//! name, time x 1e3, rate x 1e3]) and `blend` (node index -> children weights x 1e3).
use crate::session::Session;
use anyhow::{Context, Result};
use dishonored_probe::ReadPath;
use glam::{Mat4, Quat, Vec3};
use serde_json::{Map, Value, json};

const L2W: u32 = 0x60;
const SPACE_BASES: u32 = 0x208;
const ANIMATIONS: u32 = 0x1e0;
const TICK_ARRAY: u32 = 0x1a8;
const ATOM: u32 = 32;
const NODE_HEAD: (u32, u32) = (0x5c, 0x18); // flags .. NodeTotalWeight (+0x70)
const SEQ: (u32, u32) = (0xcc, 0x14); // AnimSeqName, Rate, flags, CurrentTime
const CHILDREN: u32 = 0xcc;

struct Node {
    addr: u32,
    name: String,
    class: String,
    seq: bool,
    /// (children count, element size, Weight offset).
    blend: Option<(u32, u32, u32)>,
}

pub struct AnimRec {
    pub label: String,
    comp: u32,
    bones: bool,
    world: bool,
    nbones: u32,
    names: Vec<String>,
    keep: Vec<usize>,
    nodes: Vec<Node>,
    /// Index of this recorder's first read in the script's read list.
    first: usize,
    nreads: usize,
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

impl Session {
    /// The skeletal mesh component of a subject: a component itself, or an actor's `Mesh` /
    /// `SkeletalMeshComponent` / `SkelMesh` / first SkeletalMeshComponent-valued field.
    pub fn skel_component(&mut self, obj: u32) -> Result<u32> {
        let cls = self.r.class_of(&mut self.c, obj)?;
        if self.r.field(&mut self.c, cls, "SpaceBases", false)?.is_some() {
            return Ok(obj);
        }
        for f in ["Mesh", "SkeletalMeshComponent", "SkelMesh", "SkeletalMesh"] {
            if let Ok(pl) = self.r.place(&mut self.c, obj, f) {
                let v = self.c.read_u32(pl.addr)?;
                if v != 0 && self.r.class_of(&mut self.c, v).and_then(|c| self.r.field(&mut self.c, c, "SpaceBases", false)).ok().flatten().is_some() {
                    return Ok(v);
                }
            }
        }
        anyhow::bail!("{} has no skeletal mesh component", self.c.name_of(obj).unwrap_or_default())
    }

    /// Prepare a recorder for `comp`; its reads are appended to `reads`.
    pub fn anim_rec(&mut self, label: &str, comp: u32, bones: bool, filter: &[String], reads: &mut Vec<ReadPath>) -> Result<AnimRec> {
        // `+world` in the filter adds the world-space transforms (`ws`; otherwise they follow
        // from `cs` and `l2w`).
        let world = filter.iter().any(|f| f == "+world");
        let filter: Vec<String> = filter.iter().filter(|f| *f != "+world").cloned().collect();
        let arr = self.c.read(comp + SPACE_BASES, 8)?;
        let nbones = u32_at(&arr, 4);
        let mut names = Vec::new();
        let mut keep = Vec::new();
        if bones {
            for i in 0..nbones {
                let n = self.r.call(&mut self.c, comp, "SkeletalMeshComponent.GetBoneName", &json!([i]))?["return"].as_str().unwrap_or("").to_string();
                if filter.is_empty() || filter.iter().any(|f| f.eq_ignore_ascii_case(&n)) {
                    keep.push(i as usize);
                }
                names.push(n);
            }
        }
        let mut nodes = Vec::new();
        let tree = self.c.read_u32(comp + ANIMATIONS)?;
        if tree != 0 {
            let a = self.c.read(tree + TICK_ARRAY, 8)?;
            let (data, n) = (u32_at(&a, 0), u32_at(&a, 4).min(2048));
            let list = if n > 0 { self.c.read(data, n * 4)? } else { Vec::new() };
            for k in 0..n as usize {
                let nd = u32_at(&list, k * 4);
                if nd == 0 {
                    continue;
                }
                let full = self.c.name_of(nd).unwrap_or_default();
                let class = full.split(' ').next().unwrap_or("").to_string();
                let nm = self.r.place(&mut self.c, nd, "NodeName").and_then(|pl| self.r.read_place(&mut self.c, &pl)).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
                let ncls = self.r.class_of(&mut self.c, nd)?;
                let seq = self.r.field(&mut self.c, ncls, "AnimSeqName", false)?.is_some();
                let mut blend = None;
                if !seq && self.r.field(&mut self.c, ncls, "Children", false)?.is_some() {
                    let ca = self.c.read(nd + CHILDREN, 8)?;
                    let num = u32_at(&ca, 4);
                    if num > 0 {
                        let w = self.r.place(&mut self.c, nd, "Children[0].Weight")?.addr;
                        let e0 = self.r.place(&mut self.c, nd, "Children[0]")?.addr;
                        let es = if num > 1 { self.r.place(&mut self.c, nd, "Children[1]")?.addr - e0 } else { (w - e0) + 4 };
                        blend = Some((num, es, w - e0));
                    }
                }
                nodes.push(Node { addr: nd, name: if nm == "None" { String::new() } else { nm }, class, seq, blend });
            }
        }
        let first = reads.len();
        reads.push(ReadPath { base: comp, offsets: vec![L2W], len: 64 });
        if bones {
            let total = nbones * ATOM;
            let mut off = 0;
            while off < total {
                reads.push(ReadPath { base: comp, offsets: vec![SPACE_BASES, off], len: (total - off).min(4096) });
                off += 4096;
            }
        }
        for n in &nodes {
            reads.push(ReadPath { base: n.addr, offsets: vec![NODE_HEAD.0], len: NODE_HEAD.1 });
            if n.seq {
                reads.push(ReadPath { base: n.addr, offsets: vec![SEQ.0], len: SEQ.1 });
            }
            if let Some((num, es, _)) = n.blend {
                reads.push(ReadPath { base: n.addr, offsets: vec![CHILDREN, 0], len: (num * es).min(4096) });
            }
        }
        Ok(AnimRec { label: label.into(), comp, bones, world, nbones, names, keep, nodes, first, nreads: reads.len() - first })
    }
}

impl AnimRec {
    /// Decode the run's frames (each row: every read of the script, `None` when unreadable).
    pub fn decode(&self, s: &mut Session, frames: &[Vec<Option<Vec<u8>>>], offset: [f32; 3]) -> Result<Value> {
        let mut out_frames = Vec::new();
        let mut prev_l2w: Vec<i64> = Vec::new();
        let mut prev_cs: Vec<Vec<i64>> = vec![Vec::new(); self.nbones as usize];
        let mut prev_ws: Vec<Vec<i64>> = vec![Vec::new(); self.nbones as usize];
        let mut prev_node: Vec<Value> = vec![Value::Null; self.nodes.len()];
        let mut prev_blend: Vec<Value> = vec![Value::Null; self.nodes.len()];
        let mut fnames: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
        let q = |v: f32, k: f32| (v * k).round() as i64;
        for (t, row) in frames.iter().enumerate() {
            let r = &row[self.first..(self.first + self.nreads).min(row.len())];
            let mut fr = Map::new();
            let mut k = 0;
            let l2w = r.get(k).cloned().flatten();
            k += 1;
            let m = l2w.as_ref().map(|b| {
                let f: Vec<f32> = (0..16).map(|i| f32_at(b, i * 4)).collect();
                // Row-major with row vectors: the columns of glam's matrix are the rows here.
                Mat4::from_cols_array(&f.clone().try_into().unwrap())
            });
            if let Some(b) = &l2w {
                let mut v: Vec<i64> = (0..16).map(|i| q(f32_at(b, i * 4), 1e4)).collect();
                for (i, o) in [12usize, 13, 14].iter().enumerate() {
                    v[*o] = q(f32_at(b, *o * 4) - offset[i], 100.0);
                }
                if v != prev_l2w {
                    fr.insert("l2w".into(), json!(v));
                    prev_l2w = v;
                }
            }
            if self.bones {
                let total = (self.nbones * ATOM) as usize;
                let mut sb = Vec::with_capacity(total);
                while sb.len() < total {
                    match r.get(k).cloned().flatten() {
                        Some(b) => sb.extend(b),
                        None => sb.resize(sb.len() + 4096.min(total - sb.len()), 0),
                    }
                    k += 1;
                }
                let (mut cs, mut ws) = (Map::new(), Map::new());
                for &i in &self.keep {
                    let o = i * ATOM as usize;
                    let rq = Quat::from_xyzw(f32_at(&sb, o), f32_at(&sb, o + 4), f32_at(&sb, o + 8), f32_at(&sb, o + 12));
                    let tr = Vec3::new(f32_at(&sb, o + 16), f32_at(&sb, o + 20), f32_at(&sb, o + 24));
                    let c: Vec<i64> = vec![q(rq.x, 1e4), q(rq.y, 1e4), q(rq.z, 1e4), q(rq.w, 1e4), q(tr.x, 100.0), q(tr.y, 100.0), q(tr.z, 100.0)];
                    if c != prev_cs[i] {
                        cs.insert(i.to_string(), json!(c));
                        prev_cs[i] = c;
                    }
                    if let Some(m) = m.as_ref().filter(|_| self.world) {
                        // Row-vector convention: world = bone x L2W; with glam's column
                        // vectors that is L2W^T applied on the left (m above is that transpose).
                        let p = m.transform_point3(tr) - Vec3::from(offset);
                        let (_, mr, _) = m.to_scale_rotation_translation();
                        let wq = (mr * rq).normalize();
                        let w: Vec<i64> = vec![q(wq.x, 1e4), q(wq.y, 1e4), q(wq.z, 1e4), q(wq.w, 1e4), q(p.x, 100.0), q(p.y, 100.0), q(p.z, 100.0)];
                        if w != prev_ws[i] {
                            ws.insert(i.to_string(), json!(w));
                            prev_ws[i] = w;
                        }
                    }
                }
                if !cs.is_empty() {
                    fr.insert("cs".into(), Value::Object(cs));
                }
                if !ws.is_empty() {
                    fr.insert("ws".into(), Value::Object(ws));
                }
            }
            let (mut nodes, mut blends) = (Map::new(), Map::new());
            for (ni, n) in self.nodes.iter().enumerate() {
                let head = r.get(k).cloned().flatten();
                k += 1;
                let mut v = match &head {
                    Some(b) => vec![json!(q(f32_at(b, 0x14), 1e3)), json!(u32_at(b, 0) & 1)],
                    None => vec![Value::Null],
                };
                if n.seq {
                    if let Some(b) = r.get(k).cloned().flatten() {
                        let ni_ = u32_at(&b, 0);
                        let nm = match fnames.get(&ni_) {
                            Some(x) => x.clone(),
                            None => {
                                let x = crate::reflect::name_text(&mut s.c, ni_, u32_at(&b, 4)).unwrap_or_default();
                                fnames.insert(ni_, x.clone());
                                x
                            }
                        };
                        v.extend([json!(nm), json!(q(f32_at(&b, 0x10), 1e3)), json!(q(f32_at(&b, 8), 1e3))]);
                    }
                    k += 1;
                }
                let v = Value::Array(v);
                if v != prev_node[ni] {
                    nodes.insert(ni.to_string(), v.clone());
                    prev_node[ni] = v;
                }
                if let Some((num, es, wo)) = n.blend {
                    if let Some(b) = r.get(k).cloned().flatten() {
                        let ws: Vec<i64> = (0..num as usize).filter(|c| c * es as usize + wo as usize + 4 <= b.len()).map(|c| q(f32_at(&b, c * es as usize + wo as usize), 1e3)).collect();
                        let v = json!(ws);
                        if v != prev_blend[ni] {
                            blends.insert(ni.to_string(), v.clone());
                            prev_blend[ni] = v;
                        }
                    }
                    k += 1;
                }
            }
            if !nodes.is_empty() {
                fr.insert("nodes".into(), Value::Object(nodes));
            }
            if !blends.is_empty() {
                fr.insert("blend".into(), Value::Object(blends));
            }
            if !fr.is_empty() {
                fr.insert("t".into(), json!(t));
                out_frames.push(Value::Object(fr));
            }
        }
        Ok(json!({
            "label": self.label,
            "component": crate::reflect::object_value(&mut s.c, self.comp)?,
            "units": {"quat": 1e4, "translation": 100, "weight": 1e3, "time": 1e3, "rate": 1e3, "l2w": "rotation part x 1e4, translation x 100"},
            "bones": self.keep.iter().map(|&i| json!([i, self.names.get(i)])).collect::<Vec<_>>(),
            "nodes": self.nodes.iter().map(|n| json!({"name": n.name, "class": n.class, "sequence": n.seq, "children": n.blend.map(|b| b.0)})).collect::<Vec<_>>(),
            "frames": out_frames,
        }))
    }
}

/// `session anim object=X [ticks=N] [bones=a,b|all|none] [out=FILE]`: record `ticks` ticks of
/// the subject's skeletal animation (the world runs in lockstep for the recording).
pub fn session_anim(s: &mut Session, a: &Value) -> Result<Value> {
    let spec = a.get("object").and_then(Value::as_str).context("object")?;
    let obj = s.r.object(&mut s.c, spec, None)?;
    let comp = s.skel_component(obj)?;
    let ticks = a.get("ticks").and_then(Value::as_u64).unwrap_or(60) as u32;
    let bones = a.get("bones").and_then(Value::as_str).unwrap_or("all");
    let (want, filter): (bool, Vec<String>) = match bones {
        "none" => (false, Vec::new()),
        "all" => (true, Vec::new()),
        l => (true, l.split(',').map(str::to_string).collect()),
    };
    let mut filter = filter;
    if a.get("world").and_then(Value::as_bool).unwrap_or(false) {
        filter.push("+world".into());
    }
    let mut reads = Vec::new();
    let rec = s.anim_rec(spec, comp, want, &filter, &mut reads)?;
    s.c.lockstep(true, 30_000)?;
    let (_, frames, errors) = s.c.run_script(ticks, &reads, &[], 600_000)?;
    let mut v = rec.decode(s, &frames, [0.0; 3])?;
    v["errors"] = json!(errors);
    if let Some(f) = a.get("out").and_then(Value::as_str) {
        std::fs::write(f, serde_json::to_string(&v)?)?;
        return Ok(json!({"out": f, "frames": v["frames"].as_array().map(|x| x.len()), "bones": v["bones"].as_array().map(|x| x.len()), "nodes": v["nodes"].as_array().map(|x| x.len())}));
    }
    Ok(v)
}
