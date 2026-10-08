//! Headless presentation: what the runtime would play for each tick's simulation events
//! (particle systems, sounds, animation reactions), so missing or never-ending effects show up
//! in the trace. The decisions are the runtime's own: `crates/dishonored_runtime/src/
//! present_rules.rs` is a Bevy-free module that the game's `effects`, `audio` and `characters`
//! systems call and that this file compiles directly (`#[path]`):
//!
//! - effects: `FxEventState::requests` (event -> ParticleSystem, position, direction, beams),
//!   `blink_indicator_want`, `attached_want` (trails, Dark Vision cones), and the particle
//!   simulation `step` with the one-shot end rule `one_shot_finished`, over the gameplay
//!   ParticleSystems the runtime decodes (`GAMEPLAY_PARTICLES`);
//! - sounds: `SoundEventState::posts` (cue names, contact-system sounds);
//! - animation: `collect_hits` (the reactions `npc_anim` plays).
//!
//! The session forwards every step's events to all three (`session::fixed_tick`); one rendered
//! frame per tick here. What only the Bevy side does (drawing, sockets of animated meshes) is
//! not reproduced: animation-driven sound notifies and footfalls are absent (no skeletal
//! animation runs headless, so the simulation's sword-swing cue is posted), the third-person
//! muzzle socket falls back to the eye point, placed level emitters are not simulated.

#![allow(dead_code)]

#[path = "../../../crates/dishonored_runtime/src/present_rules.rs"]
mod present_rules;

pub use present_rules::*;
use dishonored_audio::contact::ContactSystem;
use dishonored_content::MatchContent;
use dishonored_ir::particles::ParticleSystemAsset;
use dishonored_sim::events::SimEvent;
use dishonored_sim::state::{PlayerId, PlayerState};
use dishonored_world::WorldQuery;
use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// A decoded gameplay ParticleSystem with what its emitters draw with.
#[derive(Clone, Serialize, Deserialize)]
pub struct FxSystem {
    pub system: ParticleSystemAsset,
    /// Per emitter: (has a texture to draw, blend name, material path).
    pub emitters: Vec<(bool, String, String)>,
}

/// The gameplay ParticleSystems the runtime decodes with every level, by lowercase path.
#[derive(Default, Serialize, Deserialize)]
pub struct FxLibrary {
    pub systems: HashMap<String, FxSystem>,
}

const FX_CACHE_VERSION: u32 = 1;

impl FxLibrary {
    /// Decode `GAMEPLAY_PARTICLES` through the level importer (as `level.rs` passes them in
    /// `extra_particles`), cached under the oracle cache.
    pub fn load(cache_root: &std::path::Path) -> anyhow::Result<Self> {
        let key = dishonored_oracle::cache::fnv64(format!("{FX_CACHE_VERSION}:{}", GAMEPLAY_PARTICLES.join(",")).as_bytes());
        let path = cache_root.join("present").join(format!("fx-{key:016x}.bin"));
        if let Ok(b) = std::fs::read(&path) {
            if let Ok((lib, _)) = bincode::serde::decode_from_slice::<FxLibrary, _>(&b, bincode::config::standard()) {
                return Ok(lib);
            }
        }
        let content = dishonored_assets::Content::locate()?;
        let opts = dishonored_assets::level::ImportOptions {
            max_texture_size: 16,
            load_textures: true,
            load_lightmaps: false,
            other_actors: false,
            particles: true,
            extra_particles: GAMEPLAY_PARTICLES.iter().map(|s| s.to_string()).collect(),
            lightmap_cache: None,
        };
        let (map, _) = dishonored_assets::level::import_level(&content, "oracle_fx", &[], &opts)?;
        let mut lib = FxLibrary::default();
        for s in &map.particle_systems {
            let emitters = s
                .emitters
                .iter()
                .map(|e| {
                    // `effects::material_for`: a texture from the diffuse, a texture parameter, or
                    // the opacity / emissive texture; mesh emitters draw their mesh.
                    let ma = e.material.and_then(|m| map.materials.get(m));
                    let tex = ma.is_some_and(|m| m.diffuse.is_some() || !m.texture_params.is_empty()) || e.material.and_then(|m| map.material_params.get(m)).is_some_and(|p| p.opacity.is_some() || p.emissive.is_some()) || e.mesh_id.is_some();
                    (tex, format!("{:?}", e.blend), ma.map(|m| m.source.clone()).unwrap_or_default())
                })
                .collect();
            lib.systems.insert(s.source.to_ascii_lowercase(), FxSystem { system: s.clone(), emitters });
        }
        std::fs::create_dir_all(path.parent().unwrap()).ok();
        std::fs::write(&path, bincode::serde::encode_to_vec(&lib, bincode::config::standard())?).ok();
        Ok(lib)
    }
}

/// The original contact system (impact particles and sounds), loaded once per process.
pub fn contact_system() -> Option<Arc<ContactSystem>> {
    static CS: std::sync::OnceLock<Option<Arc<ContactSystem>>> = std::sync::OnceLock::new();
    CS.get_or_init(|| {
        let content = dishonored_assets::Content::locate().ok()?;
        let game = content.package("DishonoredGame").ok()?;
        let gc = content.package("DishonoredGameContent").ok();
        ContactSystem::load(&game, &gc.iter().map(|p| &**p).collect::<Vec<_>>()).map(Arc::new)
    })
    .clone()
}

/// One presentation event of a tick.
#[derive(Debug, Clone)]
pub struct PresentEvent {
    /// `Effect`, `EffectEnd`, `EffectMissing`, `Sound`, `Anim`.
    pub kind: &'static str,
    pub object: PlayerId,
    pub name: String,
    pub detail: String,
}

/// What owns a persistent (not one-shot) instance.
#[derive(Clone, Copy, PartialEq)]
enum Owner {
    /// The Blink target indicator.
    Indicator,
    /// `attached_want`: (0, projectile) trails, (1, pawn) Dark Vision cones.
    Attached((u8, u32)),
}

struct LiveFx {
    inst: u32,
    owner: Option<Owner>,
    one_shot: bool,
    path: String,
    transform: Mat4,
    sims: Vec<EmitterSim>,
}

/// The presentation state of one run.
pub struct Presenter {
    lib: Arc<FxLibrary>,
    rng: u64,
    fx: FxEventState,
    sound: SoundEventState,
    contact: Option<Arc<ContactSystem>>,
    content: Arc<MatchContent>,
    local: PlayerId,
    live: Vec<LiveFx>,
    next_inst: u32,
    now: f32,
}

impl Presenter {
    pub fn new(lib: Arc<FxLibrary>, content: Arc<MatchContent>, local: PlayerId) -> Self {
        Presenter { lib, rng: 0x1234_5678, fx: FxEventState::default(), sound: SoundEventState::default(), contact: contact_system(), content, local, live: Vec::new(), next_inst: 0, now: 0.0 }
    }

    pub fn live_effects(&self) -> usize {
        self.live.len()
    }

    pub fn live_particles(&self) -> usize {
        self.live.iter().map(|l| l.sims.iter().map(|s| s.particles.len()).sum::<usize>()).sum()
    }

    /// Start an instance of a system (`spawn_effect` / `spawn_beam` / `spawn_instance`); false
    /// when the runtime did not decode it.
    fn start(&mut self, path: &str, xf: Mat4, owner: Option<Owner>, who: PlayerId, out: &mut Vec<PresentEvent>) -> bool {
        let at = xf.w_axis.truncate();
        let Some(sys) = self.lib.systems.get(&path.to_ascii_lowercase()) else {
            out.push(PresentEvent { kind: "EffectMissing", object: who, name: path.to_string(), detail: format!("not decoded (GAMEPLAY_PARTICLES) at={:.0},{:.0},{:.0}", at.x, at.y, at.z) });
            return false;
        };
        self.next_inst += 1;
        let untextured = sys.emitters.iter().filter(|e| !e.0).count();
        let infinite = sys.system.emitters.iter().filter(|e| e.lifetime.as_ref().is_none_or(|l| l.eval1(0.0, 1.0) <= 0.0)).count();
        let kind = match owner {
            None => "oneshot",
            Some(Owner::Indicator) => "indicator",
            Some(Owner::Attached((0, _))) => "trail",
            Some(Owner::Attached(_)) => "attached",
        };
        out.push(PresentEvent {
            kind: "Effect",
            object: who,
            name: path.to_string(),
            detail: format!("inst={} kind={kind} at={:.0},{:.0},{:.0} emitters={} untextured={untextured} no_lifetime={infinite}", self.next_inst, at.x, at.y, at.z, sys.system.emitters.len()),
        });
        let sims = sys.system.emitters.iter().map(|_| EmitterSim::default()).collect();
        self.live.push(LiveFx { inst: self.next_inst, owner, one_shot: owner.is_none(), path: path.to_string(), transform: xf, sims });
        true
    }

    /// Remove an owned instance at once (`despawn`).
    fn kill(&mut self, owner: Owner, out: &mut Vec<PresentEvent>) {
        if let Some(i) = self.live.iter().position(|l| l.owner == Some(owner)) {
            let l = self.live.remove(i);
            out.push(PresentEvent { kind: "EffectEnd", object: 0, name: l.path, detail: format!("inst={} killed", l.inst) });
        }
    }

    /// One frame: the tick's events through the runtime's rules (one-shots and beams), the Blink
    /// indicator and the attached effects, the particle step, the sounds and the reactions.
    pub fn frame(&mut self, events: &[SimEvent], world: &dishonored_sim::state::WorldState, reveals: Vec<PlayerId>, dt: f32) -> Vec<PresentEvent> {
        let mut out = Vec::new();
        let players = &world.players;
        let me = players.iter().find(|p| p.id == self.local).copied();
        let others: Vec<PlayerState> = players.iter().filter(|p| p.id != self.local).copied().collect();
        let content = self.content.clone();
        let (muzzle, bones) = (HashMap::new(), HashMap::new());
        let scene = FxScene { players, me: Some(self.local), muzzle: &muzzle, bones: &bones, contact: self.contact.as_deref(), world: Some(&content.collision), now: self.now };
        for r in self.fx.requests(events, &scene) {
            match r {
                FxRequest::Effect { path, at, dir } => {
                    let who = nearest(players, at);
                    self.start(&path, facing(at, dir), None, who, &mut out);
                }
                FxRequest::Beam { path, from, to } => {
                    let who = nearest(players, from);
                    self.start(path, facing(from, to - from), None, who, &mut out);
                }
            }
        }
        // The Blink indicator: one instance following the target while aiming.
        let want = blink_indicator_want(me.as_ref(), &content.collision, &content.sim.gameplay.movement);
        let cur = self.live.iter().position(|l| l.owner == Some(Owner::Indicator));
        match (want, cur) {
            (None, Some(_)) => self.kill(Owner::Indicator, &mut out),
            (Some((path, at)), Some(i)) if self.live[i].path == path => self.live[i].transform = Mat4::from_translation(at),
            (Some((path, at)), _) => {
                self.kill(Owner::Indicator, &mut out);
                self.start(path, Mat4::from_translation(at), Some(Owner::Indicator), self.local, &mut out);
            }
            (None, None) => {}
        }
        // Attached effects: trails stop and fade out when their projectile is gone, cones are
        // killed with the power.
        let want = attached_want(me.as_ref(), &others, &world.projectiles, &reveals);
        let owned: Vec<(u8, u32)> = self
            .live
            .iter()
            .filter_map(|l| match l.owner {
                Some(Owner::Attached(k)) => Some(k),
                _ => None,
            })
            .collect();
        for k in owned.into_iter().filter(|k| !want.iter().any(|w| w.0 == *k)) {
            if k.0 == 0 {
                if let Some(l) = self.live.iter_mut().find(|l| l.owner == Some(Owner::Attached(k))) {
                    // `FxInstance::stop`: no more spawning, gone once its particles die.
                    l.owner = None;
                    l.one_shot = true;
                    for s in &mut l.sims {
                        s.done = true;
                    }
                }
            } else {
                self.kill(Owner::Attached(k), &mut out);
            }
        }
        for (k, path, m) in want {
            match self.live.iter_mut().find(|l| l.owner == Some(Owner::Attached(k))) {
                Some(l) => l.transform = m,
                None => {
                    let who = if k.0 == 1 { k.1 } else { self.local };
                    self.start(path, m, Some(Owner::Attached(k)), who, &mut out);
                }
            }
        }
        // Particles (`simulate`): every live instance, then the one-shot end rule.
        let cw = &content.collision;
        let ray_closure = |a: Vec3, b: Vec3| -> Option<(Vec3, Vec3)> {
            let d = b - a;
            let len = d.length();
            if len < 1e-4 {
                return None;
            }
            cw.raycast(a, d / len, len, dishonored_world::channel::ALL).map(|h| (h.point, h.normal))
        };
        let ray_fn: RayFn = &ray_closure;
        let rng = &mut self.rng;
        let lib = &self.lib;
        let mut ended = Vec::new();
        self.live.retain_mut(|l| {
            let Some(sys) = lib.systems.get(&l.path.to_ascii_lowercase()) else { return false };
            let xf = l.transform;
            let mut rnd = || next_rand(rng);
            for (em, sim) in sys.system.emitters.iter().zip(l.sims.iter_mut()) {
                step(em, sim, &xf, dt, &mut rnd, em.collision.as_ref().map(|_| ray_fn));
            }
            if l.one_shot && one_shot_finished(&l.sims) {
                ended.push((l.inst, l.path.clone()));
                false
            } else {
                true
            }
        });
        for (inst, path) in ended {
            out.push(PresentEvent { kind: "EffectEnd", object: 0, name: path, detail: format!("inst={inst}") });
        }
        // Sounds.
        let now = self.now;
        self.sound.recent.retain(|_, t| now - *t < 1.0);
        for ev in events {
            for (name, at) in self.sound.posts(ev, players, Some(self.local), self.contact.as_deref(), &content.collision, now) {
                let name = if name.is_empty() { "(no cue)".to_string() } else { name };
                let (object, detail) = match at {
                    SoundAt::Pawn(id) => (id, String::new()),
                    SoundAt::Impact(p) => (0, format!("at={:.0},{:.0},{:.0}", p.x, p.y, p.z)),
                };
                out.push(PresentEvent { kind: "Sound", object, name, detail });
            }
        }
        // Animation reactions.
        let all: Vec<&PlayerState> = players.iter().collect();
        let mut hits: Vec<(u32, Vec<(HitKind, Vec3)>)> = collect_hits(events, &all).into_iter().collect();
        hits.sort_by_key(|h| h.0);
        for (id, v) in hits {
            for (k, _) in v {
                out.push(PresentEvent { kind: "Anim", object: id, name: format!("{k:?}"), detail: String::new() });
            }
        }
        self.now += dt;
        out
    }
}

fn nearest(players: &[PlayerState], at: Vec3) -> PlayerId {
    players.iter().min_by(|a, b| a.position.distance(at).total_cmp(&b.position.distance(at))).map(|p| p.id).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_compile_with_the_runtime_tables() {
        assert!(GAMEPLAY_PARTICLES.contains(&"Vfx_GamePlay.Blink.Blink_Ground_01"));
        assert!(!one_shot_finished(&[EmitterSim::default()]), "a fresh emitter (time 0, not done) is not finished");
        assert!(one_shot_finished(&[EmitterSim { done: true, ..Default::default() }]));
    }
}
