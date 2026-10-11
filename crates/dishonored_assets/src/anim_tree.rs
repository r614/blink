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

/// A static enum array (`RootBoneOption[3]`): each cooked element by its array index, the
/// class default (the enum's first value) where none is cooked.
fn enum_array<T: Copy + Default>(props: &[Property], n: &str, parse: impl Fn(&str) -> T) -> [T; 3] {
    let mut out = [T::default(); 3];
    for p in props.iter().filter(|p| p.name.eq_ignore_ascii_case(n)) {
        if let (Some(slot), Value::Enum(e) | Value::Name(e)) = (out.get_mut(p.array_index.max(0) as usize), &p.value) {
            *slot = parse(e);
        }
    }
    out
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
    /// `Engine.Default__ArkAnimNodeLocomotion`: the tunables a tree's locomotion node leaves at
    /// the class default (`NPC_Human_at`'s overrides only `m_fTurnFastBlendDuration`).
    loco_defaults: Vec<Property>,
    nodes_by_export: HashMap<usize, usize>,  // export index -> node index
    nodes: Vec<AnimNode>,
}

impl NodeDecoder {
    fn new(pkg: std::sync::Arc<ue3_package::Package>, loco_defaults: Vec<Property>) -> Self {
        Self {
            pkg,
            loco_defaults,
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
        let props = decode_node_props(&self.pkg, &class, &h.properties, &self.loco_defaults);

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

/// Decode the node-class fields that drive evaluation. Property names as cooked (`obj_dump`
/// of the `NPC_Human_at` nodes, 2026-10-10): UE3's own names on the engine classes
/// (`AnimSeqName`, `BranchStartBoneName`, `bForceLocalSpaceBlend`...), Arkane's `m_` names on
/// the fields they added.
fn decode_node_props(pkg: &Package, class: &str, props: &[Property], loco_defaults: &[Property]) -> NodeProps {
    match class {
        // `AnimNodeSequence` and Arkane's `DishonoredAnimNodeSeq*` subclasses (the NPC tree's
        // loops and slot channels, the player's state sequences, the mantle sequence).
        c if c == "AnimNodeSequence" || c.contains("AnimNodeSeq") => NodeProps::Sequence {
            seq: name_of(props, "AnimSeqName"),
            // `Rate` is the class default (1) except on the reversed locomotion slots (-1).
            rate: find(props, "Rate").and_then(|p| p.as_float()).unwrap_or(1.0),
            looping: bool_of(props, "bLooping"),
            playing: bool_of(props, "bPlaying"),
            no_notifies: bool_of(props, "bNoNotifies"),
            sync_group: name_of(props, "SynchGroupName"),
            // `Engine.Default__AnimNodeSequence` sets `bSynchronize`: only an explicit false is
            // cooked (the idle slot of `LocoBlender`).
            synchronize: !matches!(find(props, "bSynchronize").map(|p| &p.value), Some(Value::Bool(false))),
            reverse_sync: bool_of(props, "bReverseSync"),
            root_bone: enum_array(props, "RootBoneOption", |e| match e {
                "RBA_Translate" => RootBoneOption::Translate,
                "RBA_Discard" => RootBoneOption::Discard,
                _ => RootBoneOption::Default,
            }),
            root_rotation: enum_array(props, "RootRotationOption", |e| match e {
                "RRO_Discard" => RootRotationOption::Discard,
                "RRO_Extract" => RootRotationOption::Extract,
                _ => RootRotationOption::Default,
            }),
            keep_undefined: bool_of(props, "m_bDontUseRefPoseOnNonDefinedBones"),
        },
        "AnimNodeBlendPerBone" | "DisAnimNodeBlendPerBone" => NodeProps::BlendPerBone {
            branches: names(pkg, props, "BranchStartBoneName"),
            local_space: bool_of(props, "bForceLocalSpaceBlend"),
            exclude_branch: bool_of(props, "m_bExcludeBranchBones"),
        },
        // `bEarlyAnimEndNotify` is true in `Engine.Default__AnimNodeSlot`; only a false is cooked.
        "DisAnimNodeSlot" | "AnimNodeSlot" => NodeProps::Slot { early_anim_end: !matches!(find(props, "bEarlyAnimEndNotify").map(|p| &p.value), Some(Value::Bool(false))) },
        "AnimNodeAdditiveBlending" => NodeProps::AdditiveBlending,
        "AnimNodeBlendByPhysics" => NodeProps::BlendByPhysics,
        "AnimNodeBlendList" => NodeProps::BlendList,
        // `Anims[]`: the 3x3 head grid, then the 3x3 torso grid (`ADD_Empty_AimHeadIdle_UpLeft`
        // ... `ADD_Empty_AimTorsoIdle_DownRight`; ANIMATION.md section 9).
        "ArkAnimNodeLookAt" => NodeProps::AimOffset { grids: structs(pkg, props, "Anims").iter().map(|a| name_of(a, "AnimName")).collect() },
        // The player's state trees: other AnimTree assets the node instantiates at run time.
        c if c.contains("AnimNodeStatePicker") || c.contains("AnimNodeTreeRef") => NodeProps::TreeRef {
            templates: structs(pkg, props, "m_PossibleTemplates")
                .iter()
                .map(|t| {
                    let path = find(t, "m_pAnimTree_Template").and_then(|p| p.as_object()).map(|i| pkg.obj_path(ObjRef::from_index(i))).unwrap_or_default();
                    (name_of(t, "m_AnimTree_Name"), path)
                })
                .collect(),
        },
        // The velocity smoothing and modifier crossfade tunables (ANIMATION.md section 3), the
        // node's value over the class default.
        "ArkAnimNodeLocomotion" => {
            let f = |n: &str| find(props, n).or_else(|| find(loco_defaults, n)).and_then(|p| p.as_float()).unwrap_or(0.0);
            NodeProps::Locomotion { rate: f("m_fVelocityMaxBlendSpeed"), range: f("m_fVelocityMaxDif"), min_ratio: f("m_fVelocityMinBlendSpeedRatio"), modifier_blend: f("m_fModifierBlendDuration") }
        }
        _ => NodeProps::Other,
    }
}

/// Load an `AnimTree` asset from its package and object path.
pub fn load_anim_tree(content: &Content, pkg: &str, path: &str) -> Result<AnimTreeAsset> {
    let pkg_arc = content.package(pkg)?;

    let tree_idx = find_export(&pkg_arc, path)
        .with_context(|| format!("AnimTree {} not found in {}", path, pkg))?;
    // The trees are package-named objects (`Ply_Player_at.Ply_Player_at`): a bare package name
    // finds the package's own export, which is no tree.
    let tree_class = pkg_arc.export_class(tree_idx);
    anyhow::ensure!(tree_class.ends_with("AnimTree"), "{path} in {pkg} is a {tree_class}, not an AnimTree");

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

    let loco_defaults = content.package("Engine").ok().and_then(|e| find_export(&e, "Default__ArkAnimNodeLocomotion").and_then(|d| e.read_object(d).ok()).map(|h| h.properties)).unwrap_or_default();
    let mut decoder = NodeDecoder::new(pkg_arc.clone(), loco_defaults);

    let root = if let Some(e) = root_export {
        Some(decoder.decode_node(e)?)
    } else {
        None
    };

    // Skeletal control lists: `SkelControlLists[]` of `BoneName` and `ControlHead`, each control
    // naming the next in its chain (`NextControl`) and itself (`ControlName`).
    let mut skel_controls = Vec::new();
    for skel_entry in structs(&pkg_arc, &h.properties, "SkelControlLists") {
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
            let ctrl_name = match name_of(&ctrl_h.properties, "ControlName") {
                n if !n.is_empty() && n != "None" => n,
                _ => {
                    let p = pkg_arc.obj_path(ObjRef::Export(ctrl_idx));
                    p.split('.').last().unwrap_or(&p).to_string()
                }
            };
            controls.push((ctrl_class, ctrl_name));

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
