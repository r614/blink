//! Placed ambient emitters (`AkAmbientSound` / `AkAmbientSoundMovable` actors) of a map.

use ue3_package::properties::find;
use ue3_package::{ObjRef, Package};

#[derive(Debug, Clone)]
pub struct Emitter {
    /// AkEvent object name (its Wwise id is `fnv1_32(name)`).
    pub event: String,
    /// Dishonored world space (Unreal units, Z-up).
    pub position: [f32; 3],
    pub auto_play: bool,
    pub actor: String,
}

/// (event name, bank package name) for every `AkEvent` exported by a package, from
/// its `RequiredBank` property.
pub fn ak_event_banks(pkg: &Package) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "AkEvent" {
            continue;
        }
        let Ok(h) = pkg.read_object(i) else { continue };
        if let Some(b) = find(&h.properties, "RequiredBank").and_then(|p| p.as_object()) {
            if b != 0 {
                out.push((pkg.obj_name(ObjRef::Export(i)), pkg.obj_name(ObjRef::from_index(b))));
            }
        }
    }
    out
}

/// Whether a placed emitter's event is a continuous ambience (looped by the runtime)
/// rather than a scripted one-shot (doors, drawbridges, footsteps placed for cutscenes).
/// Heuristic on the event naming convention, see research/dishonored/AUDIO_FORMAT.md.
pub fn is_ambience(event: &str) -> bool {
    let l = event.to_ascii_lowercase();
    l.starts_with("amb_") || l.starts_with("snd_amb_") || l.contains("loop")
}

pub fn ambient_emitters(pkg: &Package) -> Vec<Emitter> {
    let mut out = Vec::new();
    for i in 0..pkg.exports.len() {
        let class = pkg.export_class(i);
        if class != "AkAmbientSound" && class != "AkAmbientSoundMovable" {
            continue;
        }
        let name = pkg.obj_name(ObjRef::Export(i));
        if name.starts_with("Default__") {
            continue;
        }
        let Ok(h) = pkg.read_object(i) else { continue };
        let Some(ev) = find(&h.properties, "PlayEvent").and_then(|p| p.as_object()) else { continue };
        if ev == 0 {
            continue;
        }
        let Some(pos) = find(&h.properties, "Location").and_then(|p| p.as_vector()) else { continue };
        let auto_play = match find(&h.properties, "bAutoPlay").map(|p| &p.value) {
            Some(ue3_package::properties::Value::Bool(b)) => *b,
            _ => true,
        };
        out.push(Emitter {
            event: pkg.obj_name(ObjRef::from_index(ev)),
            position: pos,
            auto_play,
            actor: pkg.obj_path(ObjRef::Export(i)),
        });
    }
    out
}
