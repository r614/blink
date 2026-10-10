//! A melee item's swing trail: the first of its melee extents (`DisTweaks_InventoryItem.
//! m_MeleeCollisionInfo`, `DisMeleeExtentTweak`) on the item mesh's sockets whose
//! `m_ParticleSystemComponent` has a template. While a `DisNotify_Trails` window runs, the item
//! marks its extent caches (`m_NeedTrailUpdate`: set by 0x00C057F0, cleared by 0x00C058B0), and
//! each record of an extent its running melee context uses (0x00C061F0) feeds the component — an
//! AnimTrail — the extent's two corner sockets with their tangents (0x00C02090 -> 0x008AB910).

use crate::tweaks::{TVal, TweakDb, get, get_obj};
use anyhow::Result;

#[derive(Debug, Clone, PartialEq)]
pub struct MeleeTrail {
    /// The component's `Template` (a ParticleSystem path).
    pub template: String,
    /// The extent's `m_MeleeExtent_Socket_BL` and `m_MeleeExtent_Socket_UR` (item mesh sockets):
    /// the trail's two edges.
    pub sockets: [String; 2],
}

/// The trail of an item tweak, if one of its socket extents has a particle component.
pub fn melee_trail_in(db: &TweakDb, item: &str) -> Result<Option<MeleeTrail>> {
    let it = db.resolve(item)?;
    let Some(TVal::List(extents)) = get(&it, "m_MeleeCollisionInfo") else { return Ok(None) };
    for e in extents {
        let flag = |k: &str| matches!(e.get(k), Some(TVal::B(true)));
        if flag("m_bIgnoreSockets") || flag("m_bExtentOnPawnMesh") {
            continue;
        }
        let name = |k: &str| match e.get(k) {
            Some(TVal::Name(n)) if n != "None" => Some(n.clone()),
            _ => None,
        };
        let (Some(bl), Some(ur)) = (name("m_MeleeExtent_Socket_BL"), name("m_MeleeExtent_Socket_UR")) else { continue };
        let Some(TVal::Obj(c)) = e.get("m_ParticleSystemComponent") else { continue };
        if c.is_empty() || c == "None" {
            continue;
        }
        // The component template's own properties (its class defaults carry no template).
        let Ok(comp) = db.delta(c) else { continue };
        let Some(template) = get_obj(&comp, "Template").filter(|t| !t.is_empty() && *t != "None") else { continue };
        return Ok(Some(MeleeTrail { template: template.to_string(), sockets: [bl, ur] }));
    }
    Ok(None)
}
