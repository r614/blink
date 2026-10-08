//! Headless presentation: what the runtime would play for each tick's simulation events
//! (particle systems, sounds, animation inputs), so missing or never-ending effects show up in
//! the trace. The event handling is the runtime's own code, compiled here against stand-ins
//! for its Bevy resources (`build.rs` extracts it from `crates/dishonored_runtime/src`):
//!
//! - effects: `effects::gameplay_effects` (event -> ParticleSystem, position, direction) and the
//!   particle simulation `effects::step` with `simulate`'s one-shot despawn rule, over the
//!   gameplay ParticleSystems the runtime decodes (`GAMEPLAY_PARTICLES`);
//! - sounds: the simulation-event part of `audio::drive` (cue names, contact-system sounds);
//! - animation: `characters::collect_hits` (the hit reactions `npc_anim` plays).
//!
//! The session forwards every step's events to both (`session::fixed_tick`: `audio_events`
//! and `anim_events`); one rendered frame per tick here.
//!
//! Not covered (approximations of the shim, reported in ORACLE.md): animation-driven sound
//! notifies and footfalls (no skeletal animation runs headless, so `notify_driven` stays empty
//! and the simulation's sword-swing cue is posted), the third-person muzzle socket (no
//! animated pistol: the eye-height fallback is used), placed level emitters.

#![allow(dead_code)]

use dishonored_audio::contact::ContactSystem;
use dishonored_audio::cues;
use dishonored_audio::engine::ObjId;
use dishonored_content::MatchContent;
use dishonored_ir::particles::{ParticleEmitterAsset, ParticleSystemAsset};
use dishonored_sim::events::{DamageCause, SimEvent};
use dishonored_sim::input::LeftHand;
use dishonored_sim::state::{MoveMode, PawnKind, PlayerId, PlayerState, ProjectileState};
use dishonored_world::WorldQuery;
use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

include!(concat!(env!("OUT_DIR"), "/present_extract.rs"));

/// Stand-in for `effects::Effects`: what `gameplay_effects` touches.
#[derive(Default)]
pub struct FxShim {
    rng: u64,
    bolts: HashMap<u32, bool>,
    delayed: Vec<(f32, String, Vec3, Vec3)>,
    /// Placed Spring Razors: trap id -> position.
    traps: HashMap<u32, Vec3>,
    /// Spawn requests of this frame: (system path, position, direction).
    spawned: Vec<(String, Vec3, Vec3)>,
}

/// Stand-in for `session::SessionView`.
#[derive(Default)]
pub struct SessionView {
    pub me: Option<PlayerState>,
    pub others: Vec<PlayerState>,
    pub projectiles: Vec<ProjectileState>,
    pub dark_vision_reveals: Vec<PlayerId>,
}

/// Stand-in for `session::Session`.
pub struct Session {
    pub content: Arc<MatchContent>,
    pub local: Option<PlayerId>,
}

impl Session {
    pub fn local_id(&self) -> Option<PlayerId> {
        self.local
    }
}

/// Stand-in for `audio::GameAudio`.
#[derive(Default)]
pub struct AudioShim {
    notify_driven: HashMap<PlayerId, f32>,
    projectiles: HashMap<u32, bool>,
    notify_recent: HashMap<(PlayerId, String), f32>,
    contact: Option<Arc<ContactSystem>>,
    next_temp: u64,
}

impl AudioShim {
    pub fn contact(&self) -> Option<&ContactSystem> {
        self.contact.as_deref()
    }
    fn temp_obj(&mut self, _e: &mut EngineShim, _pos: Vec3, _now: f32) -> ObjId {
        self.next_temp += 1;
        TEMP_OBJ + self.next_temp
    }
    fn post(&mut self, e: &mut EngineShim, event: &str, obj: ObjId) {
        e.posts.push((event.to_string(), obj));
    }
}

/// Stand-in for the Wwise engine: the posted events.
#[derive(Default)]
pub struct EngineShim {
    posts: Vec<(String, ObjId)>,
}

/// `effects::spawn_effect` without the drawing: records the request.
fn spawn_effect(_commands: &mut (), _meshes: &mut (), fx: &mut FxShim, path: &str, at: Vec3, dir: Vec3) -> bool {
    fx.spawned.push((path.to_string(), at, dir));
    true
}

/// Stand-in for `effects::FxInstance` (what the despawn rule reads).
pub struct FxInstanceShim {
    one_shot: bool,
    sims: Vec<EmitterSim>,
}

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
    /// `attached_effects`: (0, projectile) trails, (1, pawn) Dark Vision cones.
    Attached((u8, u32)),
}

struct LiveFx {
    inst: u32,
    owner: Option<Owner>,
    path: String,
    transform: Mat4,
    shim: FxInstanceShim,
}

/// The presentation state of one run.
pub struct Presenter {
    lib: Arc<FxLibrary>,
    fx: FxShim,
    audio: AudioShim,
    sockets: EffectSockets,
    session: Session,
    live: Vec<LiveFx>,
    next_inst: u32,
    now: f32,
}

impl Presenter {
    pub fn new(lib: Arc<FxLibrary>, content: Arc<MatchContent>, local: PlayerId) -> Self {
        Presenter {
            lib,
            fx: FxShim { rng: 0x1234_5678, ..Default::default() },
            audio: AudioShim { contact: contact_system(), ..Default::default() },
            sockets: EffectSockets::default(),
            session: Session { content, local: Some(local) },
            live: Vec::new(),
            next_inst: 0,
            now: 0.0,
        }
    }

    pub fn live_effects(&self) -> usize {
        self.live.len()
    }

    pub fn live_particles(&self) -> usize {
        self.live.iter().map(|l| l.shim.sims.iter().map(|s| s.particles.len()).sum::<usize>()).sum()
    }

    /// Start an instance of a system (`spawn_effect` / `spawn_instance`); None when the runtime
    /// did not decode it.
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
        let one_shot = owner.is_none();
        self.live.push(LiveFx { inst: self.next_inst, owner, path: path.to_string(), transform: xf, shim: FxInstanceShim { one_shot, sims: sys.system.emitters.iter().map(|_| EmitterSim::default()).collect() } });
        true
    }

    /// Remove an owned instance at once (`despawn`).
    fn kill(&mut self, owner: Owner, out: &mut Vec<PresentEvent>) {
        if let Some(i) = self.live.iter().position(|l| l.owner == Some(owner)) {
            let l = self.live.remove(i);
            out.push(PresentEvent { kind: "EffectEnd", object: 0, name: l.path, detail: format!("inst={} killed", l.inst) });
        }
    }

    /// One frame: the tick's events through the runtime's handlers (one-shots), the Blink
    /// indicator and the attached effects (trails, Dark Vision cones), then the particle step.
    pub fn frame(&mut self, events: &[SimEvent], world: &dishonored_sim::state::WorldState, reveals: Vec<PlayerId>, dt: f32) -> Vec<PresentEvent> {
        let mut out = Vec::new();
        let local = self.session.local;
        let players = &world.players;
        let view = SessionView {
            me: players.iter().find(|p| Some(p.id) == local).copied(),
            others: players.iter().filter(|p| Some(p.id) != local).copied().collect(),
            projectiles: world.projectiles.clone(),
            dark_vision_reveals: reveals,
        };
        // One-shot effects.
        gameplay_effects_body((), (), &mut self.fx, &view, Some(&self.audio), &self.sockets, Some(&self.session), self.now, events.to_vec());
        for (path, at, dir) in std::mem::take(&mut self.fx.spawned) {
            let who = nearest(players, at);
            self.start(&path, effect_transform(at, dir), None, who, &mut out);
        }
        // The Blink indicator: one instance following the target while aiming.
        let me = local.unwrap_or(0);
        let want = blink_indicator_want(Some(&view), Some(&self.session));
        let cur = self.live.iter().position(|l| l.owner == Some(Owner::Indicator));
        match (want, cur) {
            (None, Some(_)) => self.kill(Owner::Indicator, &mut out),
            (Some((path, at)), Some(i)) if self.live[i].path == path => self.live[i].transform = Mat4::from_translation(at),
            (Some((path, at)), _) => {
                self.kill(Owner::Indicator, &mut out);
                self.start(path, Mat4::from_translation(at), Some(Owner::Indicator), me, &mut out);
            }
            (None, None) => {}
        }
        // Attached effects: trails stop and fade out when their projectile is gone, cones are
        // killed with the power.
        let want = attached_want(&view);
        let owned: Vec<(u8, u32)> = self.live.iter().filter_map(|l| match l.owner {
            Some(Owner::Attached(k)) => Some(k),
            _ => None,
        }).collect();
        for k in owned.into_iter().filter(|k| !want.iter().any(|w| w.0 == *k)) {
            if k.0 == 0 {
                if let Some(l) = self.live.iter_mut().find(|l| l.owner == Some(Owner::Attached(k))) {
                    // `FxInstance::stop`: no more spawning, gone once its particles die.
                    l.owner = None;
                    l.shim.one_shot = true;
                    for s in &mut l.shim.sims {
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
                    self.start(path, m, Some(Owner::Attached(k)), if k.0 == 1 { k.1 } else { me }, &mut out);
                }
            }
        }        // Particles (`simulate`): every live one-shot instance, then the despawn rule.
        let world = &self.session.content.collision;
        let ray_closure = |a: Vec3, b: Vec3| -> Option<(Vec3, Vec3)> {
            let d = b - a;
            let len = d.length();
            if len < 1e-4 {
                return None;
            }
            world.raycast(a, d / len, len, dishonored_world::channel::ALL).map(|h| (h.point, h.normal))
        };
        let ray_fn: RayFn = &ray_closure;
        let fx = &mut self.fx;
        let lib = &self.lib;
        let mut ended = Vec::new();
        self.live.retain_mut(|l| {
            let Some(sys) = lib.systems.get(&l.path.to_ascii_lowercase()) else { return false };
            let xf = l.transform;
            let mut rnd = || fx.rand();
            for (em, sim) in sys.system.emitters.iter().zip(l.shim.sims.iter_mut()) {
                step(em, sim, &xf, dt, &mut rnd, em.collision.as_ref().map(|_| ray_fn));
            }
            if despawned(&l.shim) {
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
        self.audio.notify_recent.retain(|_, t| self.now - *t < 1.0);
        let e = audio_events_body(&mut self.audio, EngineShim::default(), &view, &self.session, local, self.now, events.to_vec());
        for (ev, obj) in e.posts {
            let object = if obj >= PLAYER_OBJ && obj < TEMP_OBJ { (obj - PLAYER_OBJ) as PlayerId } else { 0 };
            let name = if ev.is_empty() { "(no cue)".to_string() } else { ev };
            out.push(PresentEvent { kind: "Sound", object, name, detail: if obj >= TEMP_OBJ { "at=impact".into() } else { String::new() } });
        }
        // Animation inputs.
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
    fn extracted_tables_present() {
        assert!(GAMEPLAY_PARTICLES.contains(&"Vfx_GamePlay.Blink.Blink_Ground_01"));
        assert!(PLAYER_OBJ < TEMP_OBJ);
        let inst = FxInstanceShim { one_shot: true, sims: vec![EmitterSim::default()] };
        assert!(!despawned(&inst), "a fresh emitter (time 0, not done) is not despawned");
        let done = FxInstanceShim { one_shot: true, sims: vec![EmitterSim { done: true, ..Default::default() }] };
        assert!(despawned(&done));
    }
}
