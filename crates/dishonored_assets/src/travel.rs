//! Level travel as the original wires it (see research/dishonored/BOYLE_MISSION.md, section 1):
//!
//! 1. a `DishonoredUsableObject` (door tweak `LTD_*`) fires `DisSeqEvent_Used(Originator)`;
//! 2. Kismet sets `DisSeqAct_SetPlayerTravelDestination(m_Tag = PlayerStart_*)` and activates
//!    a `ChangeLvl_*` remote event (possibly through other remote events and story-flag checks);
//! 3. the always-loaded global Kismet in `DishonoredGameFull_P` listens for that event and
//!    runs a sequence with `SeqAct_PrepareMapChange(MainLevelName)` -> `DisSeqAct_SaveLevelState`
//!    -> `SeqAct_CommitMapChange`;
//! 4. on arrival `DisSeqAct_GotoPlayerTravelDestination` places the player at the `PlayerStart`
//!    whose `Tag` matches.
//!
//! This module recovers that table from the data by walking the same graph.

use crate::content::Content;
use crate::kismet::{KismetGraph, KismetOp, read_kismet};
use anyhow::Result;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use ue3_package::ObjRef;
use ue3_package::properties::{Value, find};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TravelLink {
    /// The usable object (door) that starts the travel: (package, actor name).
    pub door_package: String,
    pub door: String,
    pub door_position: Vec3,
    /// Its usable tweak object path (`Boyle_Twk.Door_Transitions.LTD_Garden_to_Ballroom`):
    /// its localized `m_InteractText` is the original prompt.
    pub door_tweak: String,
    /// `SetPlayerTravelDestination` tag (`PlayerStart_Ballroom`).
    pub destination_tag: String,
    /// The remote event handled by the global Kismet (`ChangeLvl_fromBoyleExterior_toBoyleInterior`).
    pub change_event: String,
    /// Destination persistent map from the global `SeqAct_PrepareMapChange`.
    pub destination_map: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaggedStart {
    pub package: String,
    pub name: String,
    pub tag: String,
    pub position: Vec3,
    pub yaw: f32,
}

fn prop<'a>(op: &'a KismetOp, key: &str) -> Option<&'a str> {
    op.props.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// `Name("X")` / `-> A.B.X` -> `X` (last path element for object refs).
fn unwrap_value(v: &str) -> String {
    let v = v.trim();
    if let Some(inner) = v.strip_prefix("Name(\"").and_then(|s| s.strip_suffix("\")")) {
        return inner.to_string();
    }
    if let Some(path) = v.strip_prefix("-> ") {
        return path.rsplit('.').next().unwrap_or(path).to_string();
    }
    v.to_string()
}

/// Kismet of all packages of a map, with an index of remote-event listeners.
struct MapKismet {
    graphs: Vec<(String, KismetGraph)>,
    listeners: BTreeMap<String, Vec<(usize, usize)>>,
}

impl MapKismet {
    fn new(content: &Content, packages: &[String]) -> Self {
        let mut graphs = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for p in packages {
            if !content.has_package(p) || !seen.insert(p.to_ascii_lowercase()) {
                continue;
            }
            if let Ok(pkg) = content.package(p) {
                graphs.push((p.clone(), read_kismet(&pkg)));
            }
        }
        let mut listeners: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
        for (gi, (_, g)) in graphs.iter().enumerate() {
            for (oi, op) in g.ops.iter().enumerate() {
                if op.class == "SeqEvent_RemoteEvent" {
                    if let Some(n) = prop(op, "EventName") {
                        listeners.entry(unwrap_value(n).to_ascii_lowercase()).or_default().push((gi, oi));
                    }
                }
            }
        }
        MapKismet { graphs, listeners }
    }

    fn op(&self, at: (usize, usize)) -> &KismetOp {
        &self.graphs[at.0].1.ops[at.1]
    }

    fn by_export(&self, graph: usize, export: usize) -> Option<(usize, usize)> {
        self.graphs[graph].1.ops.iter().position(|o| o.export == export).map(|i| (graph, i))
    }

    /// Successors: output links, remote events to their listeners, and a nested sequence's
    /// activation event.
    fn next(&self, at: (usize, usize)) -> Vec<(usize, usize)> {
        let op = self.op(at);
        let mut out = Vec::new();
        for (_, targets) in &op.outputs {
            for (t, _) in targets {
                if let Some(n) = self.by_export(at.0, *t) {
                    let target = self.op(n);
                    if target.kind() == "sequence" {
                        // Entering a sub-sequence: continue from its SequenceActivated events.
                        let path = target.path.clone();
                        for (i, o) in self.graphs[at.0].1.ops.iter().enumerate() {
                            if o.sequence == path && o.class == "SeqEvent_SequenceActivated" {
                                out.push((at.0, i));
                            }
                        }
                    }
                    out.push(n);
                }
            }
        }
        if op.class == "SeqAct_ActivateRemoteEvent" {
            if let Some(n) = prop(op, "EventName") {
                if let Some(ls) = self.listeners.get(&unwrap_value(n).to_ascii_lowercase()) {
                    out.extend(ls.iter().copied());
                }
            }
        }
        out
    }
}

/// Travel doors of a map and their destinations (global Kismet resolves the map).
pub fn extract_travel(content: &Content, packages: &[String]) -> Result<Vec<TravelLink>> {
    let k = MapKismet::new(content, packages);
    let global = MapKismet::new(content, &["DishonoredGameFull_P".to_string()]);
    let mut links = Vec::new();
    for (gi, (pkg_name, g)) in k.graphs.iter().enumerate() {
        for (oi, op) in g.ops.iter().enumerate() {
            if op.class != "DisSeqEvent_Used" {
                continue;
            }
            let Some(door) = prop(op, "Originator").map(unwrap_value) else { continue };
            // Breadth-first along the graph until the ChangeLvl remote event.
            let mut queue = VecDeque::from([(gi, oi)]);
            let mut visited = std::collections::BTreeSet::new();
            let (mut tag, mut change) = (None, None);
            while let Some(at) = queue.pop_front() {
                if !visited.insert(at) || visited.len() > 400 {
                    continue;
                }
                let o = k.op(at);
                if o.class == "DisSeqAct_SetPlayerTravelDestination" && tag.is_none() {
                    tag = prop(o, "m_Tag").map(unwrap_value);
                }
                if o.class == "SeqAct_ActivateRemoteEvent" {
                    if let Some(n) = prop(o, "EventName").map(unwrap_value) {
                        if n.to_ascii_lowercase().starts_with("changelvl") {
                            change = Some(n);
                        }
                    }
                }
                if tag.is_some() && change.is_some() {
                    break;
                }
                queue.extend(k.next(at));
            }
            let (Some(tag), Some(change)) = (tag, change) else { continue };
            // Global Kismet: ChangeLvl_* -> sequence -> SeqAct_PrepareMapChange(MainLevelName).
            let mut dest = None;
            if let Some(ls) = global.listeners.get(&change.to_ascii_lowercase()) {
                let mut q: VecDeque<(usize, usize)> = ls.iter().copied().collect();
                let mut seen = std::collections::BTreeSet::new();
                while let Some(at) = q.pop_front() {
                    if !seen.insert(at) || seen.len() > 200 {
                        continue;
                    }
                    let o = global.op(at);
                    if o.class == "SeqAct_PrepareMapChange" {
                        dest = prop(o, "MainLevelName").map(unwrap_value);
                        break;
                    }
                    q.extend(global.next(at));
                }
            }
            let (pos, tweak) = actor_info(content, packages, &door).unwrap_or((Vec3::ZERO, String::new()));
            if links.iter().any(|l: &TravelLink| l.door == door && l.destination_tag == tag) {
                continue;
            }
            links.push(TravelLink { door_package: pkg_name.clone(), door, door_position: pos, door_tweak: tweak, destination_tag: tag, change_event: change, destination_map: dest });
        }
    }
    Ok(links)
}

/// Location and usable tweak name of a level actor (searched across the map's packages).
fn actor_info(content: &Content, packages: &[String], name: &str) -> Option<(Vec3, String)> {
    for p in packages {
        let Ok(pkg) = content.package(p) else { continue };
        for i in 0..pkg.exports.len() {
            if pkg.obj_name(ObjRef::Export(i)) != name || pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) != "PersistentLevel" {
                continue;
            }
            let Ok(h) = pkg.read_object(i) else { continue };
            let pos = find(&h.properties, "Location").and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
            let tweak = match find(&h.properties, "m_pUsableObjTweaks").map(|p| &p.value) {
                Some(Value::Object(o)) => pkg.obj_path(ObjRef::from_index(*o)),
                _ => String::new(),
            };
            return Some((pos, tweak));
        }
    }
    None
}

/// `PlayerStart` actors with their `Tag` (the travel destinations).
pub fn tagged_starts(content: &Content, packages: &[String]) -> Vec<TaggedStart> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for p in packages {
        if !content.has_package(p) || !seen.insert(p.to_ascii_lowercase()) {
            continue;
        }
        let Ok(pkg) = content.package(p) else { continue };
        for i in 0..pkg.exports.len() {
            if pkg.export_class(i) != "PlayerStart" {
                continue;
            }
            let Ok(h) = pkg.read_object(i) else { continue };
            let tag = match find(&h.properties, "Tag").map(|p| &p.value) {
                Some(Value::Name(n)) => n.clone(),
                _ => continue,
            };
            let position = find(&h.properties, "Location").and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
            let yaw = find(&h.properties, "Rotation").and_then(|p| p.as_rotator()).map(|r| r[1] as f32 * 360.0 / 65536.0).unwrap_or(0.0);
            out.push(TaggedStart { package: p.clone(), name: pkg.obj_name(ObjRef::Export(i)).to_string(), tag, position, yaw });
        }
    }
    out
}
