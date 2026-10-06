//! Dishonored's contact system (`DishonoredContactSystem`, object
//! `DishonoredGame.Phy_ContactSystem.ContactSystem.Dis_ContactSystem`): a table of
//! (striker contact type, struck contact type) intersections, each with sub-intersections
//! (sound cue, particle, AI noise, rumble, camera effect, decal, camera shake). Footfalls,
//! landings, weapon and projectile impacts all go through it: the striker is the pawn's
//! footfall / weapon / projectile type, the struck is the contact type of the surface's
//! physical material (`DisContactPhysicalMaterial.m_pContactType`).
//!
//! Lookup walks both class hierarchies from the most derived class up (the table is authored
//! on base classes such as `DisContactType_Footfall` / `DisContactType_Environment`); each
//! sub-intersection kind resolves separately to the most specific pair that sets it
//! (LIKELY, U-162: the native flattening `m_bFlattened` was not decompiled).

use std::collections::HashMap;
use ue3_package::properties::{Value, find};
use ue3_package::{ObjRef, Package};

#[derive(Debug, Clone, Default)]
pub struct ContactSound {
    /// AkEvent name.
    pub event: String,
    pub scale_with_speed: bool,
    pub min_speed: f32,
    pub max_speed: f32,
    pub min_volume_scale: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Intersection {
    pub striker: String,
    pub struck: String,
    pub sound: Option<ContactSound>,
    /// Volume settings without an event (a scaling override on a base pair).
    pub sound_volume: Option<ContactSound>,
    /// ParticleSystem path of the particle sub-intersection.
    pub particle: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ContactSystem {
    pub intersections: Vec<Intersection>,
    /// Contact type class -> super class.
    pub parents: HashMap<String, String>,
    index: HashMap<(String, String), usize>,
}

fn class_name(pkg: &Package, idx: i32) -> Option<String> {
    (idx != 0).then(|| pkg.obj_name(ObjRef::from_index(idx)))
}

impl ContactSystem {
    /// Read the table from `DishonoredGame.upk`; `class_pkgs` supply the class hierarchy
    /// (DishonoredGame and DishonoredGameContent).
    pub fn load(game: &Package, class_pkgs: &[&Package]) -> Option<Self> {
        let sys = (0..game.exports.len()).find(|&i| game.export_class(i) == "DishonoredContactSystem" && !game.obj_path(ObjRef::Export(i)).starts_with("Default__"))?;
        let h = game.read_object(sys).ok()?;
        let mut cs = ContactSystem::default();
        for e in find(&h.properties, "m_Intersections")?.array_structs(game).ok()? {
            let (Some(striker), Some(struck)) = (
                find(&e, "m_pStriker").and_then(|p| p.as_object()).and_then(|o| class_name(game, o)),
                find(&e, "m_pStruck").and_then(|p| p.as_object()).and_then(|o| class_name(game, o)),
            ) else {
                continue;
            };
            let mut it = Intersection { striker, struck, ..Default::default() };
            for s in find(&e, "m_SubIntersections").and_then(|p| p.array_structs(game).ok()).unwrap_or_default() {
                let class = find(&s, "m_pClass").and_then(|p| p.as_object()).and_then(|o| class_name(game, o)).unwrap_or_default();
                let Some(obj) = find(&s, "m_pSubIntersectObj").and_then(|p| p.as_object()).filter(|o| *o > 0) else { continue };
                let Ok(sh) = game.read_object(obj as usize - 1) else { continue };
                let p = &sh.properties;
                match class.as_str() {
                    "DisContactSub_SoundCue" => {
                        let mut snd = ContactSound { max_speed: 1000.0, min_volume_scale: 0.3, ..Default::default() };
                        if let Some(Value::Struct(_, v)) = find(p, "m_VolumeSettings").map(|p| &p.value) {
                            snd.scale_with_speed = matches!(find(v, "m_bScaleWithSpeed").map(|p| &p.value), Some(Value::Bool(true)));
                            snd.min_speed = find(v, "m_fMinSpeed").and_then(|p| p.as_float()).unwrap_or(0.0);
                            snd.max_speed = find(v, "m_fMaxSpeed").and_then(|p| p.as_float()).unwrap_or(1000.0);
                            snd.min_volume_scale = find(v, "m_fMinVolumeScale").and_then(|p| p.as_float()).unwrap_or(0.3);
                        }
                        match find(p, "m_pSoundEvent").and_then(|p| p.as_object()).and_then(|o| class_name(game, o)) {
                            Some(ev) => it.sound = Some(ContactSound { event: ev, ..snd }),
                            None => it.sound_volume = Some(snd),
                        }
                    }
                    "DisContactSub_Particle" => {
                        it.particle = p
                            .iter()
                            .find(|x| matches!(x.value, Value::Object(o) if o != 0) && x.name.to_ascii_lowercase().contains("particle"))
                            .and_then(|x| x.as_object())
                            .map(|o| game.obj_path(ObjRef::from_index(o)));
                    }
                    _ => {}
                }
            }
            cs.index.insert((it.striker.clone(), it.struck.clone()), cs.intersections.len());
            cs.intersections.push(it);
        }
        for pkg in std::iter::once(game).chain(class_pkgs.iter().copied()) {
            for (i, e) in pkg.exports.iter().enumerate() {
                if pkg.export_class(i) != "Class" || e.super_ == 0 {
                    continue;
                }
                let name = pkg.obj_name(ObjRef::Export(i));
                if !name.starts_with("DisContact") {
                    continue;
                }
                let sup = if e.super_ > 0 { pkg.obj_name(ObjRef::Export(e.super_ as usize - 1)) } else { pkg.obj_name(ObjRef::Import((-e.super_ - 1) as usize)) };
                cs.parents.insert(name, sup);
            }
        }
        Some(cs)
    }

    /// The class and its ancestors, most derived first.
    pub fn lineage(&self, class: &str) -> Vec<String> {
        let mut v = vec![class.to_string()];
        let mut c = class.to_string();
        while let Some(p) = self.parents.get(&c) {
            if v.contains(p) || !p.starts_with("DisContact") {
                break;
            }
            v.push(p.clone());
            c = p.clone();
        }
        v
    }

    /// The original lookup (`FUN_00be61a0` -> `FUN_00be4b10`): the exact (striker, struck) pair
    /// first; otherwise, over the striker lineage (depth i) and the struck lineage (depth j),
    /// the pair with the smallest i + j that has the sub-intersection, ties going to the
    /// shallower striker (the search walks striker depths in order and only replaces on a
    /// strictly smaller sum). Each sub-intersection slot resolves separately this way when the
    /// table is not flattened.
    fn resolve<T>(&self, striker: &str, struck: &str, f: impl Fn(&Intersection) -> Option<T>) -> Option<T> {
        if let Some(x) = self.index.get(&(striker.to_string(), struck.to_string())).and_then(|i| f(&self.intersections[*i])) {
            return Some(x);
        }
        let ss = self.lineage(striker);
        let ts = self.lineage(struck);
        let mut best: Option<(usize, T)> = None;
        for (i, s) in ss.iter().enumerate() {
            for (j, t) in ts.iter().enumerate() {
                if best.as_ref().is_some_and(|b| i + j >= b.0) {
                    break;
                }
                if let Some(x) = self.index.get(&(s.clone(), t.clone())).and_then(|k| f(&self.intersections[*k])) {
                    best = Some((i + j, x));
                }
            }
        }
        best.map(|b| b.1)
    }

    /// Sound of a contact (event + volume scaling).
    /// The sound slot resolves like every slot (nearest pair with a `DisContactSub_SoundCue`);
    /// a sound sub-intersection without an event plays nothing (0x00BC6290 returns when
    /// `m_pSoundEvent` is null), so it silences the pairs below it.
    pub fn sound(&self, striker: &str, struck: &str) -> Option<ContactSound> {
        self.resolve(striker, struck, |i| i.sound.clone().or_else(|| i.sound_volume.clone())).filter(|s| !s.event.is_empty())
    }

    pub fn particle(&self, striker: &str, struck: &str) -> Option<String> {
        self.resolve(striker, struck, |i| i.particle.clone())
    }
}

impl ContactSound {
    /// Volume scale for a contact speed (0x00BC6290): 1 unless `m_bScaleWithSpeed`; then no
    /// sound at or below `m_fMinSpeed`, else speed / `m_fMaxSpeed` clamped to
    /// [`m_fMinVolumeScale`, 1]. Posted as the `Scale_RTPC` game parameter on the sound's object
    /// (0x00BA95C0).
    pub fn volume_scale(&self, speed: f32) -> Option<f32> {
        if !self.scale_with_speed {
            return Some(1.0);
        }
        if speed <= self.min_speed {
            return None;
        }
        Some((speed / self.max_speed.max(1e-3)).clamp(self.min_volume_scale, 1.0))
    }
}
