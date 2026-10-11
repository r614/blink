//! Sound notifies of AnimSequences: `AnimNotify_AkEvent` (posts an AkEvent on the mesh owner,
//! at `BoneName` when set, with `VolumeMultiplier` / `PitchMultiplier` sent as the RTPCs
//! "<event>v" / "<event>p": `FUN_00716bd0`) and `DishonoredNotify_Footfall` (`m_iFootIndex`:
//! a footfall contact under that foot, at most one per foot every 0.25 s: `FUN_00b66ed0`).
//! Times are in sequence seconds (before `RateScale`).

use std::collections::HashMap;
use ue3_package::properties::{Value, find};
use ue3_package::{ObjRef, Package};

#[derive(Debug, Clone, PartialEq)]
pub enum NotifyKind {
    AkEvent { event: String, bone: Option<String>, volume: f32, pitch: f32, follow: bool },
    Footfall { foot: i32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SoundNotify {
    pub time: f32,
    pub kind: NotifyKind,
}

#[derive(Debug, Clone, Default)]
pub struct ClipNotifies {
    pub length: f32,
    pub rate: f32,
    pub notifies: Vec<SoundNotify>,
}

/// Sequence name -> sound notifies, for one AnimSet export.
pub fn read_anim_set(pkg: &Package, set: usize) -> HashMap<String, ClipNotifies> {
    let mut out = HashMap::new();
    let Ok(sh) = pkg.read_object(set) else { return out };
    let Some(seqs) = find(&sh.properties, "Sequences").and_then(|p| p.array_objects().ok()) else { return out };
    for s in seqs {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { continue };
        let Ok(h) = pkg.read_object(e) else { continue };
        let name = match find(&h.properties, "SequenceName").map(|p| &p.value) {
            Some(Value::Name(n)) => n.clone(),
            _ => continue,
        };
        let mut c = ClipNotifies {
            length: find(&h.properties, "SequenceLength").and_then(|p| p.as_float()).unwrap_or(0.0),
            rate: find(&h.properties, "RateScale").and_then(|p| p.as_float()).unwrap_or(1.0),
            notifies: Vec::new(),
        };
        for ev in find(&h.properties, "Notifies").and_then(|p| p.array_structs(pkg).ok()).unwrap_or_default() {
            let time = find(&ev, "Time").and_then(|p| p.as_float()).unwrap_or(0.0);
            let Some(ObjRef::Export(ni)) = find(&ev, "Notify").and_then(|p| p.as_object()).map(ObjRef::from_index) else { continue };
            let class = pkg.export_class(ni);
            let props = pkg.read_object(ni).map(|h| h.properties).unwrap_or_default();
            let kind = match class.as_str() {
                "AnimNotify_AkEvent" => {
                    let Some(ev) = find(&props, "AkEvent").and_then(|p| p.as_object()).filter(|o| *o != 0) else { continue };
                    NotifyKind::AkEvent {
                        event: pkg.obj_name(ObjRef::from_index(ev)),
                        bone: match find(&props, "BoneName").map(|p| &p.value) {
                            Some(Value::Name(n)) if n != "None" => Some(n.clone()),
                            _ => None,
                        },
                        volume: find(&props, "VolumeMultiplier").and_then(|p| p.as_float()).unwrap_or(1.0),
                        pitch: find(&props, "PitchMultiplier").and_then(|p| p.as_float()).unwrap_or(1.0),
                        follow: matches!(find(&props, "bFollowActor").map(|p| &p.value), Some(Value::Bool(true))),
                    }
                }
                "DishonoredNotify_Footfall" => NotifyKind::Footfall { foot: match find(&props, "m_iFootIndex").map(|p| &p.value) { Some(Value::Int(i)) => *i, _ => 0 } },
                _ => continue,
            };
            c.notifies.push(SoundNotify { time, kind });
        }
        if !c.notifies.is_empty() {
            out.insert(name, c);
        }
    }
    out
}

/// Every AnimSet of a package.
pub fn read_package(pkg: &Package) -> HashMap<String, ClipNotifies> {
    let mut out = HashMap::new();
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) == "AnimSet" && !pkg.obj_path(ObjRef::Export(i)).starts_with("Default__") {
            for (k, v) in read_anim_set(pkg, i) {
                out.entry(k).or_insert(v);
            }
        }
    }
    out
}

impl ClipNotifies {
    /// Notifies with time in [from, to) (sequence seconds), the original's half-open window
    /// (`UAnimNodeSequence` 0x00598440 -> 0x00599A50, research/dishonored/systems/tick_order.md
    /// section 4); a looping clip that wrapped passes `to < from` and gets both ends.
    pub fn window(&self, from: f32, to: f32) -> Vec<&SoundNotify> {
        if to >= from {
            self.notifies.iter().filter(|n| n.time >= from && n.time < to).collect()
        } else {
            self.notifies.iter().filter(|n| n.time >= from || n.time < to).collect()
        }
    }
}
