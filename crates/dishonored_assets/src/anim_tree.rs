//! Decoding of cooked UE3 `AnimTree` assets into an open node-graph IR.

use crate::content::Content;
use anyhow::{Context, Result};
use dishonored_ir::anim_tree::*;
use std::collections::HashMap;
use ue3_package::properties::{Property, Value, find};
use ue3_package::reader::Reader;
use ue3_package::{FName, ObjRef, Package};

fn f32_of(props: &[Property], n: &str) -> f32 {
    find(props, n).and_then(|p| p.as_float()).unwrap_or(0.0)
}

fn bool_of(props: &[Property], n: &str) -> bool {
    matches!(find(props, n).map(|p| &p.value), Some(Value::Bool(true)))
}

fn name_of(props: &[Property], n: &str) -> String {
    match find(props, n).map(|p| &p.value) {
        Some(Value::Name(s)) => s.clone(),
        _ => String::new(),
    }
}

/// An `array<name>` property.
fn names(pkg: &Package, props: &[Property], n: &str) -> Vec<String> {
    let Some((count, raw)) = find(props, n).and_then(|p| p.array_raw()) else { return Vec::new() };
    let mut r = Reader::new(raw);
    (0..count.min(4096)).filter_map(|_| FName::read(&mut r).ok().map(|f| pkg.name(f))).collect()
}

fn structs(pkg: &Package, props: &[Property], n: &str) -> Vec<Vec<Property>> {
    find(props, n).and_then(|p| p.array_structs(pkg).ok()).unwrap_or_default()
}

fn find_export(pkg: &Package, path: &str) -> Option<usize> {
    (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path))
}

/// Format every property for the `raw` list: (name, printed value).
fn format_property(pkg: &Package, p: &Property) -> String {
    match &p.value {
        Value::Bool(b) => format!("{}", b),
        Value::Byte(b) => format!("{}", b),
        Value::Int(i) => format!("{}", i),
        Value::Float(f) => format!("{}", f),
        Value::Name(n) => n.clone(),
        Value::Enum(e) => e.clone(),
        Value::Str(s) => s.clone(),
        Value::Object(idx) => {
            if let ObjRef::Export(e) = ObjRef::from_index(*idx) {
                pkg.obj_path(ObjRef::Export(e))
            } else {
                format!("#{}", idx)
            }
        }
        Value::Vector(v) => format!("[{},{},{}]", v[0], v[1], v[2]),
        Value::Rotator(r) => format!("({},{},{})", r[0], r[1], r[2]),
        Value::Color(c) => format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]),
        Value::LinearColor(c) => format!("[{},{},{},{}]", c[0], c[1], c[2], c[3]),
        Value::Vector2D(v) => format!("[{},{}]", v[0], v[1]),
        Value::Guid(g) => format!("{:x}-{:x}-{:x}-{:x}", g[0], g[1], g[2], g[3]),
        Value::Struct(_name, _raw) => "[struct]".to_string(),
        Value::Array(_) => "[array]".to_string(),
        Value::Raw(_name, _raw) => "[raw]".to_string(),
    }
}

/// Collect raw properties for debugging: every property name and its printed value.
fn collect_raw(pkg: &Package, props: &[Property]) -> Vec<(String, String)> {
    props.iter().map(|p| (p.name.clone(), format_property(pkg, p))).collect()
}


/// Read a single AnimTree node by export index: decode its class, properties, children, and
/// assign a unique index in the tree's nodes array (deduped by export index).
struct NodeDecoder {
    pkg: std::sync::Arc<ue3_package::Package>,
    nodes_by_export: HashMap<usize, usize>,  // export index -> node index
    nodes: Vec<AnimNode>,
}

impl NodeDecoder {
    fn new(pkg: std::sync::Arc<ue3_package::Package>) -> Self {
        Self {
            pkg,
            nodes_by_export: HashMap::new(),
            nodes: Vec::new(),
        }
    }

    fn decode_node(&mut self, export_idx: usize) -> Result<usize> {
        // Check if we've already decoded this export.
        if let Some(node_idx) = self.nodes_by_export.get(&export_idx) {
            return Ok(*node_idx);
        }

        let h = self.pkg.read_object(export_idx)?;
        let class = self.pkg.export_class(export_idx);
        let export_path = self.pkg.obj_path(ObjRef::Export(export_idx));
        let raw = collect_raw(&self.pkg, &h.properties);

        // Determine node name: NodeName property or the export name.
        let name = name_of(&h.properties, "NodeName");
        let name = if !name.is_empty() && name != "None" {
            name
        } else {
            export_path.split('.').last().unwrap_or(&export_path).to_string()
        };

        // Decode properties based on class.
        let props = decode_node_props(&self.pkg, &class, &h.properties);

        // Decode children.
        let children_structs = structs(&self.pkg, &h.properties, "Children");
        let mut children = Vec::new();
        for child_struct in children_structs {
            let child_name = name_of(&child_struct, "Name");
            let child_weight = f32_of(&child_struct, "Weight");
            let mirror = bool_of(&child_struct, "bMirrorSkeleton");
            let additive = bool_of(&child_struct, "bIsAdditive");

            let child_idx = match find(&child_struct, "Anim").and_then(|p| p.as_object()) {
                Some(obj_ref) => match ObjRef::from_index(obj_ref) {
                    ObjRef::Export(e) => Some(e),
                    _ => None,
                },
                None => None,
            };

            let child_node = if let Some(e) = child_idx {
                Some(self.decode_node(e)?)
            } else {
                None
            };

            children.push(AnimChild {
                name: child_name,
                node: child_node,
                weight: child_weight,
                mirror,
                additive,
            });
        }

        // Register this node.
        let node_idx = self.nodes.len();
        self.nodes_by_export.insert(export_idx, node_idx);

        self.nodes.push(AnimNode {
            name,
            export: export_path,
            class,
            children,
            props,
            raw,
        });

        Ok(node_idx)
    }
}

/// Decode node properties based on class type.
fn decode_node_props(pkg: &Package, class: &str, props: &[Property]) -> NodeProps {
    match class {
        "AnimNodeSequence" => {
            let seq = name_of(props, "m_AnimSeqName");
            let rate = f32_of(props, "m_fPlayRate");
            let looping = bool_of(props, "m_bLooping");
            let playing = bool_of(props, "m_bPlaying");
            let no_notifies = bool_of(props, "m_bDontUseRefPoseOnNonDefinedBones");
            let sync_group = name_of(props, "m_SyncGroupName");
            NodeProps::Sequence {
                seq,
                rate,
                looping,
                playing,
                no_notifies,
                sync_group,
            }
        }
        "AnimNodeBlendPerBone" => {
            let mut branches = names(pkg, props, "m_PerBoneBlendBones");
            if branches.is_empty() {
                branches = names(pkg, props, "PerBoneBlendBones");
            }
            let local_space = bool_of(props, "m_bLocalSpaceBlend");
            let exclude_branch = bool_of(props, "m_bExcludeBranchBones");
            NodeProps::BlendPerBone {
                branches,
                local_space,
                exclude_branch,
            }
        }
        "DisAnimNodeSlot" | "AnimNodeSlot" => {
            let early_anim_end = bool_of(props, "m_bEarlyAnimEnd");
            NodeProps::Slot { early_anim_end }
        }
        "AnimNodeAdditiveBlending" => NodeProps::AdditiveBlending,
        "AnimNodeBlendByPhysics" => NodeProps::BlendByPhysics,
        "AnimNodeBlendList" => NodeProps::BlendList,
        "ArkAnimNodeLookAt" => {
            // Look for aim grids in properties (e.g., aim offset sequence arrays).
            let grids = Vec::new(); // TODO: decode aim grids if present
            NodeProps::AimOffset { grids }
        }
        "ArkAnimNodeLocomotion" => {
            let rate = f32_of(props, "m_fPlayRate");
            let rate = if rate == 0.0 { 2000.0 } else { rate };

            let range = f32_of(props, "m_fPlayRateRange");
            let range = if range == 0.0 { 100.0 } else { range };

            let min_ratio = f32_of(props, "m_fMinPlayRateRatio");
            let min_ratio = if min_ratio == 0.0 { 0.1 } else { min_ratio };

            let modifier_blend = f32_of(props, "m_fModifierBlendTime");

            NodeProps::Locomotion {
                rate,
                range,
                min_ratio,
                modifier_blend,
            }
        }
        _ => NodeProps::Other,
    }
}

/// Load an `AnimTree` asset from its package and object path.
pub fn load_anim_tree(content: &Content, pkg: &str, path: &str) -> Result<AnimTreeAsset> {
    let pkg_arc = content.package(pkg)?;

    let tree_idx = find_export(&pkg_arc, path)
        .with_context(|| format!("AnimTree {} not found in {}", path, pkg))?;

    let h = pkg_arc.read_object(tree_idx)?;
    let tree_name = pkg_arc.obj_path(ObjRef::Export(tree_idx));

    // Find the root node: the AnimTree's Children array contains AnimBlendChild structs;
    // the first child (if present) is the root blending node.
    let root_export = structs(&pkg_arc, &h.properties, "Children")
        .first()
        .and_then(|child_struct| find(child_struct, "Anim").and_then(|p| p.as_object()))
        .and_then(|obj_ref| match ObjRef::from_index(obj_ref) {
            ObjRef::Export(e) => Some(e),
            _ => None,
        });

    let mut decoder = NodeDecoder::new(pkg_arc.clone());

    let root = if let Some(e) = root_export {
        Some(decoder.decode_node(e)?)
    } else {
        None
    };

    // Decode skeletal control lists.
    let mut skel_controls = Vec::new();
    for skel_entry in structs(&pkg_arc, &h.properties, "m_SkelControlLists") {
        let bone = name_of(&skel_entry, "BoneName");
        if bone.is_empty() || bone == "None" {
            continue;
        }

        let mut controls = Vec::new();
        let mut control_ref = match find(&skel_entry, "ControlHead").and_then(|p| p.as_object()) {
            Some(obj_ref) => match ObjRef::from_index(obj_ref) {
                ObjRef::Export(e) => Some(e),
                _ => None,
            },
            None => None,
        };

        // Follow the NextControl chain.
        while let Some(ctrl_idx) = control_ref {
            let ctrl_h = match pkg_arc.read_object(ctrl_idx) {
                Ok(h) => h,
                Err(_) => break,
            };

            let ctrl_class = pkg_arc.export_class(ctrl_idx);
            let ctrl_name = pkg_arc.obj_path(ObjRef::Export(ctrl_idx));
            controls.push((
                ctrl_class,
                ctrl_name.split('.').last().unwrap_or(&ctrl_name).to_string(),
            ));

            // Read the next control in the chain.
            control_ref = match find(&ctrl_h.properties, "NextControl").and_then(|p| p.as_object()) {
                Some(obj_ref) => match ObjRef::from_index(obj_ref) {
                    ObjRef::Export(e) => Some(e),
                    _ => None,
                },
                None => None,
            };
        }

        if !controls.is_empty() {
            skel_controls.push(SkelControlEntry { bone, controls });
        }
    }

    Ok(AnimTreeAsset {
        name: tree_name,
        root,
        nodes: decoder.nodes,
        skel_controls,
    })
}
