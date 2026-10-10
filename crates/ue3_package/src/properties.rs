//! UE3 tagged property stream as serialized by Dishonored (file version 801).
//!
//! Tag layout: `Name:FName Type:FName Size:i32 ArrayIndex:i32`, then
//! `StructName:FName` for StructProperty, `Value:u8` for BoolProperty and
//! `EnumName:FName` for ByteProperty, then `Size` bytes of payload.
//! Array element types are not recorded in the stream, so arrays are kept raw
//! and decoded by callers that know the owning class.

use crate::reader::Reader;
use crate::{FName, ObjRef, Package};
use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub enum Value {
    Int(i32),
    Float(f32),
    Bool(bool),
    Byte(u8),
    Enum(String),
    Name(String),
    Str(String),
    Object(i32),
    Vector([f32; 3]),
    /// Pitch, Yaw, Roll in UE3 rotation units (65536 = 360 degrees).
    Rotator([i32; 3]),
    Color([u8; 4]),
    LinearColor([f32; 4]),
    Vector2D([f32; 2]),
    Guid([u32; 4]),
    Struct(String, Vec<Property>),
    /// Raw array payload (element count + elements); see [`Property::array_raw`].
    Array(Vec<u8>),
    Raw(String, Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct Property {
    pub name: String,
    pub type_name: String,
    pub array_index: i32,
    pub value: Value,
}

/// Immutable structs are serialized natively (no nested tags) in cooked packages.
fn native_struct(r: &mut Reader, name: &str, size: usize) -> Result<Option<Value>> {
    let v = match (name, size) {
        ("Vector", 12) => Value::Vector(r.vec3()?),
        ("Rotator", 12) => Value::Rotator([r.i32()?, r.i32()?, r.i32()?]),
        ("Color", 4) => {
            let b = r.array::<4>()?;
            // Stored BGRA.
            Value::Color([b[2], b[1], b[0], b[3]])
        }
        ("LinearColor", 16) => Value::LinearColor([r.f32()?, r.f32()?, r.f32()?, r.f32()?]),
        ("Vector2D", 8) => Value::Vector2D([r.f32()?, r.f32()?]),
        ("Guid", 16) => Value::Guid(r.guid()?),
        _ => return Ok(None),
    };
    Ok(Some(v))
}

pub fn read_properties(pkg: &Package, r: &mut Reader) -> Result<Vec<Property>> {
    let mut out = Vec::new();
    loop {
        let at = r.pos();
        let name_ref = FName::read(r)?;
        let name = pkg.name(name_ref);
        if name == "None" {
            break;
        }
        let type_name = pkg.name(FName::read(r)?);
        let size = r.i32()?;
        let array_index = r.i32()?;
        if !(0..=0x0400_0000).contains(&size) {
            bail!("property {name} at {at:#x}: implausible size {size}");
        }
        let size = size as usize;
        let value = match type_name.as_str() {
            "IntProperty" => Value::Int(sub(r, size)?.i32()?),
            "FloatProperty" => Value::Float(sub(r, size)?.f32()?),
            "BoolProperty" => Value::Bool(r.u8()? != 0),
            "ByteProperty" => {
                let enum_name = pkg.name(FName::read(r)?);
                let mut s = sub(r, size)?;
                if enum_name == "None" || size == 1 {
                    Value::Byte(s.u8()?)
                } else {
                    Value::Enum(pkg.name(FName::read(&mut s)?))
                }
            }
            "NameProperty" => Value::Name(pkg.name(FName::read(&mut sub(r, size)?)?)),
            "StrProperty" => Value::Str(sub(r, size)?.fstring()?),
            "ObjectProperty" | "ComponentProperty" | "ClassProperty" | "InterfaceProperty" => {
                Value::Object(sub(r, size)?.i32()?)
            }
            "StructProperty" => {
                let struct_name = pkg.name(FName::read(r)?);
                let body = r.bytes(size)?;
                let mut s = Reader::new(body);
                match native_struct(&mut s, &struct_name, size)? {
                    Some(v) => v,
                    None => match read_properties(pkg, &mut s) {
                        Ok(props) if s.pos() == size => Value::Struct(struct_name, props),
                        _ => Value::Raw(struct_name, body.to_vec()),
                    },
                }
            }
            "ArrayProperty" => Value::Array(r.bytes(size)?.to_vec()),
            other => Value::Raw(other.to_string(), r.bytes(size)?.to_vec()),
        };
        out.push(Property {
            name,
            type_name,
            array_index,
            value,
        });
    }
    Ok(out)
}

/// Take `size` bytes as an independent reader, advancing `r` past them.
fn sub<'a>(r: &mut Reader<'a>, size: usize) -> Result<Reader<'a>> {
    Ok(Reader::new(r.bytes(size)?))
}

/// Header fields every exported object starts with, followed by its properties.
pub struct ObjectHeader {
    pub net_index: i32,
    pub properties: Vec<Property>,
    /// Offset into the export data where native (class-specific) serialization begins.
    pub native_offset: usize,
}

/// RF_HasStack: object carries a script state frame before its properties.
pub const RF_HAS_STACK: u64 = 0x0200_0000;
pub const RF_CLASS_DEFAULT_OBJECT: u64 = 0x0000_0200_0000_0000;
pub const RF_ARCHETYPE_OBJECT: u64 = 0x0000_0400_0000_0000;

impl Package {
    /// Whether an export's class derives from `Component`. The class hierarchy lives in
    /// other packages, so this uses the naming convention every UE3 component class follows.
    pub fn is_component_class(&self, export: usize) -> bool {
        // UDistributionFloat/Vector derive from UComponent (observed: TemplateOwnerClass
        // prefix on DistributionFloatUniform in L_BoyleStreet_Ext_Script).
        // Engine.upk components whose names lack the suffix (their super chains reach
        // `Component`; listed because other packages only see them as imports).
        const ENGINE_COMPONENTS: &[&str] = &["ApexComponentBase", "ArkComponentContainer", "RB_Handle", "RB_Spring"];
        // Tweak classes named like components but derived from `DisTweaksBase` (an Object): seen
        // only as imports outside DishonoredGame.upk, so the suffix alone misreads them (their
        // first property was taken for a TemplateOwnerClass, e.g. `m_fDetonationDelay`, U-102).
        const NOT_COMPONENTS: &[&str] = &["DisTweaks_GrenadeComponent", "DisDLC06Tweaks_GrenadeComponent"];
        let by_name = |c: &str| !NOT_COMPONENTS.contains(&c) && (c.ends_with("Component") || c == "Component" || c.starts_with("Distribution") || ENGINE_COMPONENTS.contains(&c));
        // Classes defined in this package: walk their super chain, so components whose name
        // does not end in "Component" (`DisActivePowerComponent_Blink`, `ArkComponentContainer`,
        // `RB_Handle`) and plain objects whose name does (`DisTweaks_GrenadeComponent`,
        // `DisAnimStateComponent`) are told apart (DishonoredGame CDO subobjects, U-014).
        let mut cur = ObjRef::from_index(self.exports[export].class);
        for _ in 0..64 {
            match cur {
                ObjRef::Export(ci) => {
                    let name = self.obj_name(cur);
                    if name == "Component" || name == "ActorComponent" || name.starts_with("Distribution") && name != "Distribution" {
                        return true;
                    }
                    if name == "Object" {
                        return false;
                    }
                    let sup = ObjRef::from_index(self.exports[ci].super_);
                    if sup == ObjRef::Null {
                        return by_name(&name);
                    }
                    cur = sup;
                }
                ObjRef::Import(_) => {
                    let name = self.obj_name(cur);
                    return name != "Object" && by_name(&name);
                }
                ObjRef::Null => return false,
            }
        }
        by_name(&self.export_class(export))
    }

    /// Components owned by a class default object (directly or via a Class) are
    /// templates and serialize a TemplateName. Archetype-flagged level prefabs are not
    /// (observed: `Env_Blockout.collisions.*.StaticMeshComponent0` in L_Boyle_Ext_Env).
    pub fn is_template(&self, export: usize) -> bool {
        let mut cur = self.exports[export].outer;
        for _ in 0..32 {
            match ObjRef::from_index(cur) {
                ObjRef::Export(o) => {
                    let e = &self.exports[o];
                    if e.object_flags & RF_CLASS_DEFAULT_OBJECT != 0 || e.class == 0 {
                        return true;
                    }
                    cur = e.outer;
                }
                _ => return false,
            }
        }
        false
    }
}

impl Package {
    /// Parse the generic UObject header + tagged properties of an export.
    pub fn read_object(&self, export: usize) -> Result<ObjectHeader> {
        let data = self.export_data(export)?;
        let mut r = Reader::new(data);
        let flags = self.exports[export].object_flags;
        if flags & RF_HAS_STACK != 0 {
            // FStateFrame as observed in Dishonored (e.g. L_Boyle_Ext_Env StaticMeshActor_1737):
            // Node:i32 StateNode:i32 ProbeMask:u64 LatentAction:u16 [Offset:i32 if Node != 0].
            // No StateStack array is present at this version/licensee.
            let node = r.i32()?;
            let _state_node = r.i32()?;
            let _probe_mask = r.u64()?;
            let _latent_action = r.u16()?;
            if node != 0 {
                let _offset = r.i32()?;
            }
        }
        let class = self.export_class(export);
        // DominantPointLightComponent has no shadow map: its data starts with
        // TemplateOwnerClass = 0 and the first tag at offset 8 (L_Boyle_Ext_Light export 7).
        // Not on their class default objects (Engine `Default__DominantDirectionalLightComponent`
        // starts with the TemplateOwnerClass dword and the first tag at offset 4).
        if (class == "DominantDirectionalLightComponent" || class == "DominantSpotLightComponent") && flags & RF_CLASS_DEFAULT_OBJECT == 0 {
            // Dominant light components serialize `TArray<WORD> DominantLightShadowMap`
            // before everything else (observed in L_Boyle_Ext_Light exports 5 and 10).
            let n = r.count(2)?;
            r.skip(n * 2)?;
        }
        if self.is_component_class(export) && flags & RF_CLASS_DEFAULT_OBJECT != 0 {
            // Component class default objects: a single dword, then properties, with no
            // NetIndex (Default__Component is 4 bytes + "None"; Default__CylinderComponent
            // has its first tag at offset 4).
            let _template_owner_class = r.i32()?;
            let properties = read_properties(self, &mut r)
                .with_context(|| format!("properties of component CDO {export} ({class})"))?;
            return Ok(ObjectHeader { net_index: 0, properties, native_offset: r.pos() });
        }
        if self.is_component_class(export) {
            // UComponent: TemplateOwnerClass, plus TemplateName for class-default templates.
            // Observed in L_Boyle_Ext_Env StaticMeshComponents: i32 0 precedes NetIndex.
            let _template_owner_class = r.i32()?;
            if self.is_template(export) {
                let _template_name = FName::read(&mut r)?;
            }
        }
        let net_index = r.i32()?;
        let properties = read_properties(self, &mut r)
            .with_context(|| format!("properties of export {export} ({class})"))?;
        Ok(ObjectHeader {
            net_index,
            properties,
            native_offset: r.pos(),
        })
    }

    pub fn obj_ref_name(&self, idx: i32) -> String {
        self.obj_path(ObjRef::from_index(idx))
    }
}

pub fn find<'p>(props: &'p [Property], name: &str) -> Option<&'p Property> {
    props.iter().find(|p| p.name.eq_ignore_ascii_case(name))
}

impl Property {
    /// Decode an array of i32 object references.
    pub fn array_objects(&self) -> Result<Vec<i32>> {
        let Value::Array(b) = &self.value else {
            bail!("{} is not an array", self.name)
        };
        let mut r = Reader::new(b);
        let n = r.count(4)?;
        (0..n).map(|_| r.i32()).collect()
    }

    /// Decode an array whose elements are non-native structs (nested tagged properties).
    pub fn array_structs(&self, pkg: &Package) -> Result<Vec<Vec<Property>>> {
        let Value::Array(b) = &self.value else {
            bail!("{} is not an array", self.name)
        };
        let mut r = Reader::new(b);
        let n = r.count(8)?;
        (0..n).map(|_| read_properties(pkg, &mut r)).collect()
    }

    pub fn array_raw(&self) -> Option<(usize, &[u8])> {
        let Value::Array(b) = &self.value else {
            return None;
        };
        let n = i32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize;
        Some((n, &b[4..]))
    }

    pub fn as_object(&self) -> Option<i32> {
        match self.value {
            Value::Object(i) => Some(i),
            _ => None,
        }
    }
    pub fn as_vector(&self) -> Option<[f32; 3]> {
        match self.value {
            Value::Vector(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_rotator(&self) -> Option<[i32; 3]> {
        match self.value {
            Value::Rotator(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_float(&self) -> Option<f32> {
        match self.value {
            Value::Float(v) => Some(v),
            Value::Int(v) => Some(v as f32),
            _ => None,
        }
    }
}

pub fn format_value(pkg: &Package, v: &Value) -> String {
    match v {
        Value::Object(i) => format!("-> {}", pkg.obj_ref_name(*i)),
        Value::Struct(n, props) => {
            let inner: Vec<String> = props
                .iter()
                .map(|p| format!("{}={}", p.name, format_value(pkg, &p.value)))
                .collect();
            format!("{n}{{{}}}", inner.join(", "))
        }
        Value::Array(b) => {
            let n = b.get(..4).map(|x| i32::from_le_bytes(x.try_into().unwrap())).unwrap_or(-1);
            // An array of 8-byte elements whose first words are name indices and second small
            // instance numbers: names (`array<name>`).
            if n > 0 && b.len() == 4 + 8 * n as usize {
                let names: Option<Vec<String>> = b[4..]
                    .chunks_exact(8)
                    .map(|c| {
                        let (i, k) = (i32::from_le_bytes(c[..4].try_into().unwrap()), i32::from_le_bytes(c[4..].try_into().unwrap()));
                        (i >= 0 && (i as usize) < pkg.names.len() && (0..1000).contains(&k)).then(|| if k > 0 { format!("{}_{}", pkg.names[i as usize].name, k - 1) } else { pkg.names[i as usize].name.clone() })
                    })
                    .collect();
                if let Some(v) = names {
                    return format!("names{v:?}");
                }
            }
            format!("[{n} elems, {} bytes]", b.len())
        }
        Value::Raw(t, b) => format!("<{t} {} bytes>", b.len()),
        other => format!("{other:?}"),
    }
}
