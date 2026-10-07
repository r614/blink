//! Collision sources of a map as the original sees them (research/dishonored/COLLISION.md).
//!
//! Cooked objects store deltas against their archetype (a placed component against
//! `Engine.Default__StaticMeshActor.StaticMeshComponent0`, that against
//! `Engine.Default__StaticMeshComponent`, ...), so the effective collision flags of a placed
//! actor and of its components come from the whole archetype chain ([`PropResolver`]).
//!
//! Dishonored's collision filter (Dishonored.exe):
//! - `UPrimitiveComponent::m_CollisionTraceTypes` (`DisPrimTraceMask`, +0x140) replaces UE3's
//!   `BlockZeroExtent` / `BlockNonZeroExtent` (deprecated, never read). Bits: 0 `Move_NonPawn`,
//!   1 `Move_NonPlayerPawn`, 2 `Move_Player`, 3 `Gameplay_Crosshair`, 4 `_Projectile`,
//!   5 `_Melee`, 6 `_VisionLOS`. `SetTraceBlocking(Gameplay, Movement)` (0x0052A230) sets bits
//!   3-6 and 0-2.
//! - The bit a check tests (0x00529990): trace flag 0x8000000 VisionLOS, 0x1000000 Crosshair,
//!   0x4000000 Melee, 0x2000000 Projectile; with none of these, 0x10000000 NonPlayerPawn,
//!   0x20000000 Player, else by the source actor: none or not a pawn NonPawn, the player's pawn
//!   Player, another pawn NonPlayerPawn. Zero-extent and extent checks test the same bit.
//! - A component is checked when it collides (0x00529E90: `CollideActors` and its owner's
//!   `bCollideActors`), its mask bit is set and its owner's `ShouldTrace` (vtable +0x1C4)
//!   accepts the trace: world geometry (`bWorldGeometry`) needs `TRACE_LevelGeometry` (0x80)
//!   only, without `BlockActors`.
//! - A static mesh answers with its simplified collision (`RB_BodySetup.AggGeom`) when its
//!   `UseSimpleBoxCollision` (extent checks) or `UseSimpleLineCollision` (zero extent) is set,
//!   the component has an owner and the check does not ask for complex collision (flags
//!   0x20100); else with its kDOP triangles (`UStaticMeshComponent::LineCheck`, 0x007717D0).
//!   A mesh that wants simple collision but has no body setup is not hit.

use crate::content::Content;
use glam::Vec3;
use std::collections::HashMap;
use std::sync::Arc;
use ue3_package::properties::{Property, Value, find};
use ue3_package::{ObjRef, Package};

/// `DisPrimTraceMask` bits.
pub mod mask {
    pub const MOVE_NON_PAWN: u8 = 1 << 0;
    pub const MOVE_NPC: u8 = 1 << 1;
    pub const MOVE_PLAYER: u8 = 1 << 2;
    pub const CROSSHAIR: u8 = 1 << 3;
    pub const PROJECTILE: u8 = 1 << 4;
    pub const MELEE: u8 = 1 << 5;
    pub const VISION: u8 = 1 << 6;
    pub const MOVE: u8 = MOVE_NON_PAWN | MOVE_NPC | MOVE_PLAYER;
    pub const GAMEPLAY: u8 = CROSSHAIR | PROJECTILE | MELEE | VISION;
    pub const ALL: u8 = 0x7F;
    pub const NAMES: [&str; 7] = [
        "m_bTraceForMove_NonPawn",
        "m_bTraceForMove_NonPlayerPawn",
        "m_bTraceForMove_Player",
        "m_bTraceForGameplay_Crosshair",
        "m_bTraceForGameplay_Projectile",
        "m_bTraceForGameplay_Melee",
        "m_bTraceForGameplay_VisionLOS",
    ];
}

/// Resolves the effective properties of objects through their archetype chains.
pub struct PropResolver<'c> {
    content: &'c Content,
    paths: HashMap<String, Arc<HashMap<String, usize>>>,
    cache: HashMap<(String, usize), Arc<Vec<Property>>>,
}

/// Merge `own` over `base`: same name and index replaces, structs merge member by member.
pub fn merge_props(base: &mut Vec<Property>, own: &[Property]) {
    for p in own {
        match base.iter_mut().find(|b| b.name.eq_ignore_ascii_case(&p.name) && b.array_index == p.array_index) {
            Some(b) => match (&mut b.value, &p.value) {
                (Value::Struct(_, bs), Value::Struct(_, ps)) => merge_props(bs, ps),
                _ => *b = p.clone(),
            },
            None => base.push(p.clone()),
        }
    }
}

impl<'c> PropResolver<'c> {
    pub fn new(content: &'c Content) -> Self {
        PropResolver { content, paths: HashMap::new(), cache: HashMap::new() }
    }

    fn path_index(&mut self, pkg: &Package) -> Arc<HashMap<String, usize>> {
        let key = pkg.name.to_ascii_lowercase();
        if let Some(m) = self.paths.get(&key) {
            return m.clone();
        }
        let m: HashMap<String, usize> = (0..pkg.exports.len()).map(|i| (pkg.obj_path(ObjRef::Export(i)).to_ascii_lowercase(), i)).collect();
        let m = Arc::new(m);
        self.paths.insert(key, m.clone());
        m
    }

    /// The export an object reference names: in `pkg`, or (an import) in the package of its
    /// outermost name, or in one of `extra` packages.
    pub fn locate(&mut self, pkg: &Arc<Package>, r: ObjRef, extra: &[String]) -> Option<(Arc<Package>, usize)> {
        match r {
            ObjRef::Null => None,
            ObjRef::Export(i) => Some((pkg.clone(), i)),
            ObjRef::Import(_) => {
                let path = pkg.obj_path(r);
                let (top, rest) = path.split_once('.').unwrap_or((path.as_str(), ""));
                let mut cands: Vec<String> = vec![top.to_string()];
                cands.extend(extra.iter().cloned());
                for c in cands {
                    if !self.content.has_package(&c) {
                        continue;
                    }
                    let Ok(q) = self.content.package(&c) else { continue };
                    let idx = self.path_index(&q);
                    for p in [path.to_ascii_lowercase(), rest.to_ascii_lowercase()] {
                        if let Some(&i) = idx.get(&p) {
                            return Some((q.clone(), i));
                        }
                    }
                }
                None
            }
        }
    }

    /// The class default object an export without a serialized archetype derives from: its
    /// class's `Default__<Class>`, or for a class default object its super class's.
    fn class_default(&mut self, pkg: &Arc<Package>, i: usize) -> Option<(Arc<Package>, usize)> {
        let (cpkg, ci) = self.locate(pkg, ObjRef::from_index(pkg.exports[i].class), &[])?;
        let (cpkg, ci) = if pkg.obj_name(ObjRef::Export(i)).starts_with("Default__") {
            let sup = ObjRef::from_index(cpkg.exports[ci].super_);
            self.locate(&cpkg, sup, &[])?
        } else {
            (cpkg, ci)
        };
        let want = format!("Default__{}", cpkg.obj_name(ObjRef::Export(ci))).to_ascii_lowercase();
        let idx = self.path_index(&cpkg);
        idx.get(&want).map(|&d| (cpkg.clone(), d))
    }

    /// The object an export's property `name` refers to, from the export itself or the nearest
    /// archetype that sets it, as a full object path (`Package.Group.Name` for imports).
    pub fn inherited_object_path(&mut self, pkg: &Arc<Package>, i: usize, name: &str) -> Option<String> {
        let (mut p, mut e) = (pkg.clone(), i);
        for _ in 0..16 {
            let props = p.read_object(e).ok()?.properties;
            if let Some(o) = find(&props, name).and_then(|x| x.as_object()) {
                return (o != 0).then(|| p.obj_path(ObjRef::from_index(o)));
            }
            let arch = ObjRef::from_index(p.exports[e].archetype);
            let next = if arch != ObjRef::Null { self.locate(&p, arch, &[]) } else { self.class_default(&p, e) }?;
            if Arc::ptr_eq(&next.0, &p) && next.1 == e {
                return None;
            }
            (p, e) = next;
        }
        None
    }

    /// Effective tagged properties of an export: its own over its archetype's (recursively).
    pub fn effective(&mut self, pkg: &Arc<Package>, i: usize) -> Arc<Vec<Property>> {
        let key = (pkg.name.to_ascii_lowercase(), i);
        if let Some(p) = self.cache.get(&key) {
            return p.clone();
        }
        // Guard against cycles while resolving.
        self.cache.insert(key.clone(), Arc::new(Vec::new()));
        let arch = ObjRef::from_index(pkg.exports[i].archetype);
        let src = if arch != ObjRef::Null { self.locate(pkg, arch, &[]) } else { self.class_default(pkg, i) };
        let mut base: Vec<Property> = match src {
            Some((q, j)) if !(Arc::ptr_eq(&q, pkg) && j == i) => self.effective(&q, j).as_ref().clone(),
            _ => Vec::new(),
        };
        if let Ok(h) = pkg.read_object(i) {
            merge_props(&mut base, &h.properties);
        }
        let out = Arc::new(base);
        self.cache.insert(key, out.clone());
        out
    }
}

/// `KActor` and its Dishonored subclasses (physics props, breakables, pickups, batteries).
/// `AKActor::ShouldTrace` (0x00782D60) accepts a check with `TRACE_Movers`; without
/// `TRACE_Blocking` (the vision line, 0x08023086) whatever the actor's blocking flags, with it
/// only when the source is blocked by the actor (`IsBlockedBy`).
pub const KACTOR_CLASSES: &[&str] = &[
    "KActor",
    "KActorSpawnable",
    "DishonoredKActor",
    "DishonoredBreakable",
    "DishonoredBreakableNavBlock",
    "DishonoredMovable",
    "DisSpeaker_PA",
    "DisWhaleOilBattery",
    "DisDLC05WhaleOilBattery",
    "DisDLC05_Movable",
    "DisDLC07GravehoundSkull",
    "DisNPCAttachment",
    "DisRiverKrustBodyPart",
    "DisPickup_Base",
    "DisAbstractItemPickup",
    "DisAbstractItemPickupNote",
    "DisAbstractItemPickupAudioLog",
    "DisElixirHealth",
    "DisElixirMana",
    "DisGenericPickup",
    "DisKey_Base",
    "DisStatPickup",
    "DishonoredInventoryPickup",
    "DisWhaleBoneCharm",
    "DisDLC07WhaleBoneCharmCracked",
];

pub fn is_kactor(class: &str) -> bool {
    KACTOR_CLASSES.contains(&class)
}

pub fn flag(props: &[Property], name: &str) -> bool {
    matches!(find(props, name).map(|p| &p.value), Some(Value::Bool(true)))
}

/// `m_CollisionTraceTypes` of resolved component properties.
pub fn trace_mask(props: &[Property]) -> u8 {
    let Some(Value::Struct(_, s)) = find(props, "m_CollisionTraceTypes").map(|p| &p.value) else { return 0 };
    let mut m = 0;
    for (b, n) in mask::NAMES.iter().enumerate() {
        if flag(s, n) {
            m |= 1 << b;
        }
    }
    m
}

/// One colliding primitive of a placed actor, with its effective flags.
#[derive(Debug, Clone, Default)]
pub struct CollisionSource {
    pub package: String,
    pub actor: String,
    pub class: String,
    pub component: String,
    pub component_class: String,
    pub location: Vec3,
    /// Actor `bCollideActors`, `bBlockActors`, `bWorldGeometry`, `bHidden`, `bStatic`.
    pub actor_collide: bool,
    pub actor_block: bool,
    pub world_geometry: bool,
    pub actor_hidden: bool,
    pub actor_static: bool,
    /// `CollisionType` when serialized on the actor or its archetypes.
    pub collision_type: String,
    /// Component `CollideActors`, `BlockActors`, `HiddenGame`, `BlockRigidBody`.
    pub collide: bool,
    pub block: bool,
    pub hidden_game: bool,
    pub block_rigid_body: bool,
    pub trace_mask: u8,
    /// `StaticMesh` / `SkeletalMesh` / brush (`<brush>` with the hull count).
    pub asset: String,
    /// Arkane flags read from the actor (`bCollideOnlyWithNPCMovement`, `m_bRejectPlayerMantle`,
    /// `m_bTransparentForVisionChecks`, `m_bOverrideDefaultVisionTransparency`).
    pub extra: Vec<String>,
    /// The flags as serialized on the placed objects themselves (`None`: inherited).
    pub own_collide: Option<bool>,
    pub own_block: Option<bool>,
    pub own_actor_collide: Option<bool>,
    pub own_actor_block: Option<bool>,
    /// The actor's usable-object, breakable or door tweak (`m_pUsableObjTweaks`, `m_pBreakableTweaks`, `m_pDoorTweaks`),
    /// an object path, set or inherited.
    pub tweak: Option<String>,
}

impl CollisionSource {
    /// The component enters any collision check (0x00529E90).
    pub fn active(&self) -> bool {
        self.collide && self.actor_collide
    }
}

/// Component property names that hold primitives on an actor.
const COMPONENT_PROPS: &[&str] = &["CollisionComponent", "StaticMeshComponent", "SkeletalMeshComponent", "BrushComponent", "CylinderComponent", "LightComponent"];

/// Every primitive component of every actor placed in `packages` (`PersistentLevel` exports).
pub fn collision_sources(content: &Content, packages: &[String]) -> Vec<CollisionSource> {
    let mut r = PropResolver::new(content);
    let mut out = Vec::new();
    for name in packages {
        let Ok(pkg) = content.package(name) else { continue };
        for i in 0..pkg.exports.len() {
            if pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) != "PersistentLevel" {
                continue;
            }
            let class = pkg.export_class(i);
            if class.ends_with("Component") || class == "Model" || class == "Polys" {
                continue;
            }
            let ap = r.effective(&pkg, i);
            // Object references of inherited properties belong to the archetype's package: take the
            // component references from the placed actor's own properties.
            let own = pkg.read_object(i).map(|h| h.properties).unwrap_or_default();
            let mut comps: Vec<i32> = Vec::new();
            for n in COMPONENT_PROPS {
                if let Some(o) = find(&own, n).and_then(|p| p.as_object()) {
                    comps.push(o);
                }
            }
            for n in ["Components", "StaticMeshComponents"] {
                if let Some(v) = find(&own, n).and_then(|p| p.array_objects().ok()) {
                    comps.extend(v);
                }
            }
            let mut seen = std::collections::BTreeSet::new();
            let loc = Vec3::from(find(&ap, "Location").and_then(|p| p.as_vector()).unwrap_or([0.0; 3]));
            let tweak = ["m_pUsableObjTweaks", "m_pBreakableTweaks", "m_pDoorTweaks"].iter().find_map(|n| r.inherited_object_path(&pkg, i, n));
            let mut extra: Vec<String> = ["bCollideOnlyWithNPCMovement", "m_bRejectPlayerMantle", "m_bTransparentForVisionChecks", "m_bOverrideDefaultVisionTransparency", "bCollideWorld"]
                .iter()
                .filter(|n| flag(&ap, n))
                .map(|n| n.to_string())
                .collect();
            if let Some(Value::Enum(ph)) | Some(Value::Name(ph)) = find(&ap, "Physics").map(|p| &p.value) {
                extra.push(ph.clone());
            }
            for c in comps {
                // Components of the placed actor are exports of the same package; references to
                // class default components (never instanced in a level) are skipped.
                let ObjRef::Export(ci) = ObjRef::from_index(c) else { continue };
                if ci >= pkg.exports.len() {
                    continue;
                }
                if !seen.insert(ci) {
                    continue;
                }
                let cclass = pkg.export_class(ci);
                if !matches!(cclass.as_str(), "StaticMeshComponent" | "SkeletalMeshComponent" | "BrushComponent" | "CylinderComponent" | "SplineMeshComponent" | "InstancedStaticMeshComponent" | "FracturedStaticMeshComponent" | "DisSkeletalMeshComponent") && !cclass.contains("MeshComponent") {
                    continue;
                }
                let cp = r.effective(&pkg, ci);
                let cown = pkg.read_object(ci).map(|h| h.properties).unwrap_or_default();
                let asset = ["StaticMesh", "SkeletalMesh"]
                    .iter()
                    .find_map(|k| {
                        find(&cown, k)
                            .and_then(|p| p.as_object())
                            .filter(|o| *o != 0)
                            .map(|o| pkg.obj_path(ObjRef::from_index(o)))
                            .or_else(|| find(&cp, k).map(|_| format!("<inherited {}>", r.inherited_object_path(&pkg, ci, k).unwrap_or_default())))
                    })
                    .unwrap_or_else(|| if cclass == "BrushComponent" { "<brush>".into() } else { String::new() });
                let own_bool = |props: &[Property], n: &str| match find(props, n).map(|p| &p.value) {
                    Some(Value::Bool(b)) => Some(*b),
                    _ => None,
                };
                let enum_of = |props: &[Property], n: &str| match find(props, n).map(|p| &p.value) {
                    Some(Value::Enum(e)) | Some(Value::Name(e)) => e.clone(),
                    Some(Value::Byte(b)) => b.to_string(),
                    _ => String::new(),
                };
                out.push(CollisionSource {
                    package: pkg.name.clone(),
                    actor: pkg.obj_name(ObjRef::Export(i)),
                    class: class.clone(),
                    component: pkg.obj_name(ObjRef::Export(ci)),
                    component_class: cclass,
                    location: loc,
                    actor_collide: flag(&ap, "bCollideActors"),
                    actor_block: flag(&ap, "bBlockActors"),
                    world_geometry: flag(&ap, "bWorldGeometry"),
                    actor_hidden: flag(&ap, "bHidden"),
                    actor_static: flag(&ap, "bStatic"),
                    collision_type: enum_of(&ap, "CollisionType"),
                    collide: flag(&cp, "CollideActors"),
                    block: flag(&cp, "BlockActors"),
                    hidden_game: flag(&cp, "HiddenGame"),
                    block_rigid_body: flag(&cp, "BlockRigidBody"),
                    trace_mask: trace_mask(&cp),
                    asset,
                    extra: extra.clone(),
                    own_collide: own_bool(&cown, "CollideActors"),
                    own_block: own_bool(&cown, "BlockActors"),
                    own_actor_collide: own_bool(&own, "bCollideActors"),
                    own_actor_block: own_bool(&own, "bBlockActors"),
                    tweak: tweak.clone(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_members_merge_over_the_archetype() {
        let b = |n: &str, v: bool| Property { name: n.into(), type_name: "BoolProperty".into(), array_index: 0, value: Value::Bool(v) };
        let mut base = vec![Property {
            name: "m_CollisionTraceTypes".into(),
            type_name: "StructProperty".into(),
            array_index: 0,
            value: Value::Struct("DisPrimTraceMask".into(), mask::NAMES.iter().map(|n| b(n, true)).collect()),
        }];
        let own = vec![Property {
            name: "m_CollisionTraceTypes".into(),
            type_name: "StructProperty".into(),
            array_index: 0,
            value: Value::Struct("DisPrimTraceMask".into(), vec![b("m_bTraceForGameplay_VisionLOS", false), b("m_bTraceForGameplay_Melee", false)]),
        }];
        merge_props(&mut base, &own);
        assert_eq!(trace_mask(&base), mask::ALL & !mask::VISION & !mask::MELEE);
    }

    /// Boyle Ext: the class defaults reach the placed components (every static mesh component of
    /// a `StaticMeshActor` that does not override them collides on every trace type), and the
    /// blocking volumes answer to movement checks only.
    #[test]
    fn boyle_ext_effective_flags() {
        let Ok(c) = Content::locate() else { return };
        if !c.has_package("L_Boyle_Ext_P") {
            return;
        }
        let pk = crate::map_packages(&c, "L_Boyle_Ext_P");
        let s = collision_sources(&c, &pk);
        let bv: Vec<_> = s.iter().filter(|x| x.class == "BlockingVolume").collect();
        assert_eq!(bv.len(), 8);
        assert!(bv.iter().all(|x| x.trace_mask == mask::MOVE && x.collide && x.block && x.world_geometry && x.actor_collide), "{bv:?}");
        // The collection components that collide: on every trace type, gameplay traces only (the
        // player walks through them) and movement only (hidden collision proxies).
        let smc: Vec<_> = s.iter().filter(|x| x.class == "StaticMeshCollectionActor" && x.active() && x.world_geometry).collect();
        let count = |m: u8| smc.iter().filter(|x| x.trace_mask == m).count();
        assert_eq!(count(mask::ALL), 1669);
        assert_eq!(count(mask::GAMEPLAY), 497);
        assert_eq!(count(mask::MOVE), 148);
        // `DisStaticCollisionActor_15` collides with NPC movement only.
        let dsca: Vec<_> = s.iter().filter(|x| x.class == "DisStaticCollisionActor").collect();
        assert_eq!(dsca.len(), 1);
        assert_eq!(dsca[0].trace_mask, mask::MOVE_NPC);
        assert!(dsca[0].extra.iter().any(|e| e == "bCollideOnlyWithNPCMovement"));
    }
}
