//! [ai] Audio portal occlusion for AI hearing, driven by the doors and the Kismet: a `DisDoor`
//! with an `m_pAudioPortal` rewrites the portal's `m_fOcclusion_HeardByAI` as it moves
//! (`UpdateAudioOcclusion` 0x00A2C800: the door tweak's `m_fAISoundOcclusion` closed, 0 once
//! open by 20 degrees; the port's doors rest closed or open, so the value follows the rest
//! state), and `DisSeqAct_SetAudioOcclusion` (0x00BA1780) writes it directly.

use super::MissionMatch;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// What the current map's portals were last set from.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PortalSync {
    pub map: String,
    /// Door actor -> the state its portal was last set for.
    pub doors: BTreeMap<String, u8>,
    /// Portal name -> the Kismet value last applied.
    pub kismet: BTreeMap<String, f32>,
}

/// A portal's AI occlusion from the doors on it, each (state, occlusion while closed): an open
/// door clears it.
pub fn portal_value(doors: &[(u8, f32)]) -> f32 {
    doors.iter().map(|&(state, occ)| if state == 0 { occ } else { 0.0 }).reduce(f32::min).unwrap_or(0.0)
}

/// Before the crowd thinks: portals whose doors changed state, then the Kismet's new values.
/// A travel re-installs the cooked portals, so a new map applies everything again.
pub fn update(m: &mut MissionMatch) {
    if !m.portal_sync.map.eq_ignore_ascii_case(&m.current_map) {
        m.portal_sync = PortalSync { map: m.current_map.clone(), ..Default::default() };
    }
    let Some(level) = m.setup.ai_levels.get(&m.current_map.to_ascii_lowercase()) else { return };
    let states: Vec<(usize, &str, u8, f32)> = level
        .portal_doors
        .iter()
        .map(|d| {
            let state = m.kismet.as_ref().and_then(|k| k.door_state(&d.actor)).or_else(|| m.props.get(&d.actor).map(|p| p.door)).unwrap_or(d.initial);
            (d.portal, d.actor.as_str(), state, d.ai_occlusion)
        })
        .collect();
    let mut dirty = BTreeSet::new();
    for (portal, actor, state, _) in &states {
        if m.portal_sync.doors.get(*actor) != Some(state) {
            m.portal_sync.doors.insert(actor.to_string(), *state);
            dirty.insert(*portal);
        }
    }
    for p in dirty {
        let doors: Vec<(u8, f32)> = states.iter().filter(|s| s.0 == p).map(|s| (s.2, s.3)).collect();
        if let Some(port) = m.crowd.audio.portals.get_mut(p) {
            port.occlusion_ai = portal_value(&doors);
        }
    }
    if let Some(k) = m.kismet.as_ref() {
        for (name, v) in k.audio_occlusion_ai(&m.current_map) {
            if m.portal_sync.kismet.get(&name) != Some(&v) {
                for port in m.crowd.audio.portals.iter_mut().filter(|p| p.name.eq_ignore_ascii_case(&name)) {
                    port.occlusion_ai = v;
                }
                m.portal_sync.kismet.insert(name, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::portal_value;

    #[test]
    fn a_closed_door_occludes_and_an_open_leaf_clears_the_portal() {
        assert_eq!(portal_value(&[(0, 0.8)]), 0.8);
        assert_eq!(portal_value(&[(0, 0.8), (1, 0.8)]), 0.0);
        assert_eq!(portal_value(&[(2, 1.0)]), 0.0);
        // `m_bNoSoundOcclusion` doors carry 0.
        assert_eq!(portal_value(&[(0, 0.0)]), 0.0);
    }
}
