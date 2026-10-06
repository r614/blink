//! Game audio, decoded from the install's Wwise banks at startup and driven by
//! simulation events. Presentation only: nothing here feeds back into the sim.
//!
//! - Banks are parsed and every sound the mapping can post is decoded on a worker
//!   thread (`dishonored_audio`); playback starts once that finishes.
//! - The local player's sounds are 2D; remote players, NPCs, impacts and placed
//!   emitters are 3D: panning from bevy_audio's spatial sink (the listener is the game
//!   camera) and distance attenuation from the original Wwise attenuation curves.
//! - Ambience: the level bed and score (state events applied first) plus the map's
//!   placed `AkAmbientSound` emitters, re-picked when a pick ends (APPROXIMATION of
//!   Wwise container looping, whose v65 flags are not decoded yet).
//!
//! Flags: `--no-audio`, `--volume V` (linear master gain, default 1); env `DISHONORED_MUTE`
//! silences every sound of the process (test runs).

use crate::coords;
use crate::session::{Session, SessionView};
use bevy::audio::{
    AddAudioSource, AudioPlayer, AudioSink, AudioSinkPlayback, Decodable, PlaybackMode, PlaybackSettings, SpatialAudioSink,
    SpatialListener, SpatialScale, Volume,
};
use bevy::prelude::*;
use dishonored_audio::cues::{self, Gait};
use dishonored_audio::library::{Cue, Leaf, MediaKey, SoundLibrary};
use dishonored_audio::Pcm;
use dishonored_sim::events::{DamageCause, SimEvent};
use dishonored_sim::state::{MoveMode, PawnKind, PlayerId, PlayerState};
use dishonored_world::WorldQuery;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

type AmbientPick = (usize, Vec<(Leaf, Arc<Pcm>)>);
use std::sync::{Arc, Mutex};

/// rodio positions are scaled so every audible distance stays below 1 unit: rodio's
/// own 1/d^2 gain then clamps to 1 and only its stereo panning remains. Distance
/// attenuation is applied by [`update_voices`] from the Wwise curves.
const SPATIAL_SCALE: f32 = 0.003;
/// Fallback rolloff (uu) for 3D voices whose sound has no attenuation (2D in Wwise).
const FALLBACK_RANGE: f32 = 2500.0;
/// Placed emitters are only kept playing within this margin of their max distance.
const EMITTER_MARGIN: f32 = 500.0;

#[derive(Resource, Clone, Copy)]
pub struct AudioSettings {
    pub enabled: bool,
    /// Master gain: `--volume` x the settings global volume.
    pub volume: f32,
    /// `--volume` alone.
    pub cli: f32,
    /// Bus gains from the settings (music / SFX / voices, 0..1).
    pub music: f32,
    pub sfx: f32,
    pub voice: f32,
}

impl AudioSettings {
    pub fn from_args() -> Self {
        let a: Vec<String> = std::env::args().collect();
        let volume = a.iter().position(|x| x == "--volume").and_then(|i| a.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(1.0f32);
        Self { enabled: !a.iter().any(|x| x == "--no-audio"), volume: volume.max(0.0), cli: volume.max(0.0), music: 1.0, sfx: 1.0, voice: 1.0 }
    }
}

fn bus_gain(s: &AudioSettings, bus: u8) -> f32 {
    match bus {
        1 => s.music,
        2 => s.voice,
        _ => s.sfx,
    }
}

/// Options -> Audio volumes (global, music, SFX, voices) onto the mixer.
fn sync_volumes(gs: Option<Res<crate::settings::GameSettings>>, mut s: ResMut<AudioSettings>) {
    let Some(gs) = gs else { return };
    if !gs.is_changed() {
        return;
    }
    let a = &gs.s.audio;
    s.volume = s.cli * a.global / 100.0;
    s.music = a.music / 100.0;
    s.sfx = a.sfx / 100.0;
    s.voice = a.voices / 100.0;
}

/// Decoded PCM as a bevy_audio source.
#[derive(Asset, TypePath, Clone)]
pub struct PcmClip(pub Arc<Pcm>);

pub struct PcmDecoder {
    pcm: Arc<Pcm>,
    pos: usize,
}

impl Iterator for PcmDecoder {
    type Item = bevy::audio::Sample;
    fn next(&mut self) -> Option<Self::Item> {
        let s = self.pcm.samples.get(self.pos).copied();
        self.pos += 1;
        s
    }
}

impl bevy::audio::Source for PcmDecoder {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.pcm.samples.len().saturating_sub(self.pos))
    }
    fn channels(&self) -> bevy::audio::ChannelCount {
        std::num::NonZero::new(self.pcm.channels.max(1)).unwrap()
    }
    fn sample_rate(&self) -> bevy::audio::SampleRate {
        std::num::NonZero::new(self.pcm.sample_rate.max(1)).unwrap()
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_secs_f32(self.pcm.duration()))
    }
}

impl Decodable for PcmClip {
    type Decoder = PcmDecoder;
    fn decoder(&self) -> PcmDecoder {
        PcmDecoder { pcm: self.0.clone(), pos: 0 }
    }
}

/// Continuous ambience slot (level bed, score, placed emitter).
struct AmbientSlot {
    /// The level score (music bus) rather than ambience (SFX bus).
    music: bool,
    cue: Cue,
    /// Dishonored world position; `None` = 2D.
    pos: Option<Vec3>,
    gain_db: f32,
    /// Farthest audible distance (emitters are culled beyond it).
    range: f32,
    next_try: f32,
}

struct Loaded {
    lib: Arc<SoundLibrary>,
    cues: HashMap<String, Cue>,
    ambience: Vec<AmbientSlot>,
    report: String,
}

#[derive(Resource, Default)]
pub struct GameAudio {
    rx: Option<Mutex<Receiver<anyhow::Result<Loaded>>>>,
    lib: Option<Arc<SoundLibrary>>,
    cues: HashMap<String, Cue>,
    ambience: Vec<AmbientSlot>,
    clips: HashMap<MediaKey, Handle<PcmClip>>,
    rng: u64,
    /// Projectile id -> fired as a crossbow bolt.
    projectiles: HashMap<u32, bool>,
    steps: HashMap<PlayerId, (Vec3, f32)>,
    /// Ambience picks decoded on worker threads: (slot, leaves with PCM).
    amb_tx: Option<Sender<AmbientPick>>,
    amb_rx: Option<Mutex<Receiver<AmbientPick>>>,
    pending: Vec<bool>,
    posted: u64,
    events_seen: u64,
    last_stats: f32,
}

#[derive(Clone, Copy)]
enum Anchor {
    Player(PlayerId),
    Point(Vec3),
}

#[derive(Component)]
pub struct Voice {
    anchor: Option<Anchor>,
    gain_db: f32,
    attenuation: Option<u32>,
    max_distance: Option<f32>,
    /// Mixer bus for the settings volumes: 0 SFX, 1 music, 2 voices.
    bus: u8,
}

#[derive(Component)]
struct AmbientVoice(usize);

pub struct GameAudioPlugin;

impl Plugin for GameAudioPlugin {
    fn build(&self, app: &mut App) {
        let settings = AudioSettings::from_args();
        app.insert_resource(settings);
        if !settings.enabled {
            return;
        }
        app.add_audio_source::<PcmClip>()
            .init_resource::<GameAudio>()
            .add_systems(Startup, start_loading)
            .add_systems(Update, (sync_volumes, finish_loading, attach_listener, play_sim_events, footsteps, ambience, update_voices, stats).chain());
    }
}

fn map_arg() -> String {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == "--map").and_then(|i| a.get(i + 1)).cloned().unwrap_or_else(|| dishonored_content::DEFAULT_MAP.to_string())
}

fn start_loading(mut audio: ResMut<GameAudio>) {
    let (tx, rx) = channel();
    let map = map_arg();
    std::thread::Builder::new()
        .name("audio-load".into())
        .spawn(move || {
            let _ = tx.send(load(&map));
        })
        .ok();
    audio.rx = Some(Mutex::new(rx));
    audio.rng = 0x9e37_79b9_7f4a_7c15;
}

/// Worker thread: banks, event resolution, map emitters, decoding.
fn load(map: &str) -> anyhow::Result<Loaded> {
    let t0 = std::time::Instant::now();
    let content = dishonored_assets::Content::locate()?;
    let mut lib = SoundLibrary::new(&content.root)?;
    for b in cues::PLAYER_BANKS {
        if let Err(e) = lib.load_bank(b) {
            warn!("audio: {b}: {e:#}");
        }
    }
    let amb = cues::map_ambience(map);
    if let Some(a) = &amb {
        for b in a.banks {
            if let Err(e) = lib.load_bank(b) {
                warn!("audio: {b}: {e:#}");
            }
        }
    }
    // Placed emitters and the banks their events need.
    let mut emitters = Vec::new();
    let mut event_bank: HashMap<String, String> = HashMap::new();
    for p in dishonored_assets::map_packages(&content, map) {
        let Ok(pkg) = content.package(&p) else { continue };
        event_bank.extend(dishonored_audio::emitters::ak_event_banks(&pkg));
        emitters.extend(dishonored_audio::emitters::ambient_emitters(&pkg).into_iter().filter(|e| e.auto_play && dishonored_audio::emitters::is_ambience(&e.event)));
    }
    let mut ev_names: Vec<&String> = emitters.iter().map(|e| &e.event).collect();
    ev_names.sort();
    ev_names.dedup();
    for ev in ev_names {
        if let Some(b) = event_bank.get(ev) {
            if let Err(e) = lib.load_bank(b) {
                warn!("audio: {b}: {e:#}");
            }
        }
    }
    if let Some(a) = &amb {
        for s in a.state_events {
            lib.apply_state_event(s);
        }
    }

    let mut cues_map = HashMap::new();
    let mut missing = Vec::new();
    for ev in cues::all_player_events() {
        match lib.resolve(&ev) {
            Some(c) => {
                cues_map.insert(ev, c);
            }
            None => missing.push(ev),
        }
    }
    let mut ambience = Vec::new();
    if let Some(a) = &amb {
        if let Some(c) = lib.resolve(a.bed) {
            ambience.push(AmbientSlot { music: false, cue: c, pos: None, gain_db: 0.0, range: f32::MAX, next_try: 0.0 });
        }
        if let Some(c) = a.music.and_then(|m| lib.resolve(m)) {
            ambience.push(AmbientSlot { music: true, cue: c, pos: None, gain_db: -8.0, range: f32::MAX, next_try: 0.0 });
        }
    }
    let mut placed = 0;
    for e in &emitters {
        let Some(c) = lib.resolve(&e.event) else { continue };
        let mut leaves = Vec::new();
        c.leaves(&mut leaves);
        let range = leaves.iter().filter_map(|l| l.max_distance).fold(0.0f32, f32::max);
        let range = if range > 0.0 { range } else { FALLBACK_RANGE };
        ambience.push(AmbientSlot { music: false, cue: c, pos: Some(Vec3::from(e.position)), gain_db: 0.0, range, next_try: 0.0 });
        placed += 1;
    }

    // Decode everything that can be posted, so playback never decodes on the main thread.
    let mut keys: Vec<MediaKey> = Vec::new();
    for c in cues_map.values() {
        let mut l = Vec::new();
        c.leaves(&mut l);
        keys.extend(l.iter().map(|l| l.media));
    }
    keys.sort_by_key(|k| (k.bank, k.id));
    keys.dedup();
    let (mut ok, mut failed, mut secs) = (0, 0, 0.0f32);
    for k in &keys {
        match lib.decode(*k) {
            Ok(p) => {
                ok += 1;
                secs += p.duration();
            }
            Err(e) => {
                failed += 1;
                warn!("audio: media {} undecodable: {e:#}", k.id);
            }
        }
    }
    let report = format!(
        "audio: {} banks, {}/{} player events resolved, {} ambience slots ({placed} placed emitters), {ok} player sounds decoded ({:.0} s; ambience decodes on demand){} in {:.1} s",
        lib.banks.len(),
        cues_map.len(),
        cues_map.len() + missing.len(),
        ambience.len(),
        secs,
        if failed > 0 { format!(", {failed} failed") } else { String::new() },
        t0.elapsed().as_secs_f32()
    );
    Ok(Loaded { lib: Arc::new(lib), cues: cues_map, ambience, report })
}

fn finish_loading(mut audio: ResMut<GameAudio>) {
    let Some(rx) = &audio.rx else { return };
    let msg = rx.lock().unwrap().try_recv();
    match msg {
        Ok(Ok(l)) => {
            info!("{}", l.report);
            audio.lib = Some(l.lib);
            audio.cues = l.cues;
            audio.ambience = l.ambience;
            audio.pending = vec![false; audio.ambience.len()];
            let (tx, rx) = channel();
            audio.amb_tx = Some(tx);
            audio.amb_rx = Some(Mutex::new(rx));
            audio.rx = None;
        }
        Ok(Err(e)) => {
            warn!("audio disabled: {e:#}");
            audio.rx = None;
        }
        Err(_) => {}
    }
}

fn attach_listener(mut commands: Commands, cams: Query<Entity, (With<crate::players::GameCamera>, Without<SpatialListener>)>) {
    for e in &cams {
        // Ear gap ~ 20 cm in presentation units (1 uu = 2 cm).
        commands.entity(e).insert(SpatialListener::new(0.2));
    }
}

impl GameAudio {
    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    fn clip(&mut self, assets: &mut Assets<PcmClip>, k: MediaKey) -> Option<Handle<PcmClip>> {
        if let Some(h) = self.clips.get(&k) {
            return Some(h.clone());
        }
        let pcm = self.lib.as_ref()?.decode(k).ok()?;
        let h = assets.add(PcmClip(pcm));
        self.clips.insert(k, h.clone());
        Some(h)
    }

    /// Post an original event. `anchor: None` plays 2D (local player / UI).
    fn post(&mut self, commands: &mut Commands, assets: &mut Assets<PcmClip>, event: &str, anchor: Option<Anchor>, at: Vec3, extra_db: f32) -> usize {
        let Some(cue) = self.cues.get(event).cloned() else { return 0 };
        let mut leaves = Vec::new();
        let mut r = || self.rand();
        cue.pick(&mut r, &mut leaves);
        let n = leaves.len();
        self.posted += 1;
        debug!("post {event} ({n} voices, {})", if anchor.is_some() { "3D" } else { "2D" });
        let bus = if event.contains("VO_") { 2 } else { 0 };
        for l in leaves {
            self.spawn_leaf(commands, assets, &l, anchor, at, extra_db, bus);
        }
        n
    }

    fn spawn_leaf(&mut self, commands: &mut Commands, assets: &mut Assets<PcmClip>, l: &Leaf, anchor: Option<Anchor>, at: Vec3, extra_db: f32, bus: u8) {
        let Some(h) = self.clip(assets, l.media) else { return };
        spawn_voice(commands, h, l, anchor, at, extra_db, None, bus);
    }
}

fn db(v: f32) -> f32 {
    10f32.powf(v / 20.0)
}

fn player<'a>(view: &'a SessionView, id: PlayerId) -> Option<&'a PlayerState> {
    view.me.iter().chain(view.others.iter()).find(|p| p.id == id)
}

/// Surface under a point, from the name of the colliding mesh (APPROXIMATION).
fn surface_at(session: &Session, pos: Vec3) -> &'static str {
    let world = &session.content.collision;
    match world.raycast(pos + Vec3::Z * 10.0, -Vec3::Z, 300.0, dishonored_world::channel::ALL) {
        Some(hit) => {
            let tri = &world.tris[hit.tri as usize];
            let name = world.sources.get(tri.source as usize).map(String::as_str).unwrap_or("");
            let s = cues::surface_from_name(name);
            trace!("surface {s} <- {name}");
            s
        }
        None => "Stone",
    }
}

/// Surface of the nearest collision triangle around an impact point (short probes
/// along the six axes, since the hit normal is not part of the event).
fn surface_near(session: &Session, p: Vec3) -> &'static str {
    let world = &session.content.collision;
    let mut best: Option<(f32, u32)> = None;
    for d in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
        if let Some(hit) = world.raycast(p - d * 12.0, d, 36.0, dishonored_world::channel::ALL) {
            if best.is_none_or(|b| hit.distance < b.0) {
                best = Some((hit.distance, hit.tri));
            }
        }
    }
    let Some((_, tri)) = best else { return "Stone" };
    let src = world.tris[tri as usize].source as usize;
    cues::surface_from_name(world.sources.get(src).map(String::as_str).unwrap_or(""))
}

/// Events the online client already played from its own prediction (skip the
/// authoritative copy for the local player to avoid doubling).
pub fn predicted_locally(e: &SimEvent, me: Option<PlayerId>) -> bool {
    match e {
        SimEvent::Jumped { id } | SimEvent::BlinkStart { id, .. } | SimEvent::SwordSwing { id } | SimEvent::Fired { id, .. } | SimEvent::Landed { id, .. } => Some(*id) == me,
        _ => false,
    }
}

fn play_sim_events(
    mut commands: Commands,
    mut audio: ResMut<GameAudio>,
    mut assets: ResMut<Assets<PcmClip>>,
    mut view: ResMut<SessionView>,
    session: Res<Session>,
) {
    let events = std::mem::take(&mut view.audio_events);
    audio.events_seen += events.len() as u64;
    if audio.lib.is_none() {
        return;
    }
    let me = session.local_id();
    let a = &mut *audio;
    for e in events {
        // (event, subject player) pairs; the subject decides 2D vs 3D and the position.
        let mut posts: Vec<(String, Option<PlayerId>, Option<Vec3>, f32)> = Vec::new();
        let cue = |k: &str| cues::cue_event(k).unwrap_or("").to_string();
        match e {
            SimEvent::SwordSwing { id } => posts.push((cue("sword_swing"), Some(id), None, 0.0)),
            SimEvent::SwordBlocked { defender, .. } => posts.push((cue("sword_clash"), Some(defender), None, 0.0)),
            SimEvent::SwordParried { defender, .. } => {
                posts.push((cue("sword_clash"), Some(defender), None, 0.0));
                posts.push((cue("sword_parry_whoosh"), Some(defender), None, 0.0));
            }
            SimEvent::Fired { id, bolt, projectile } => {
                a.projectiles.insert(projectile, bolt);
                posts.push((cue(if bolt { "crossbow_fire" } else { "pistol_fire" }), Some(id), None, 0.0));
            }
            SimEvent::Reloaded { id } => posts.push((cue("pistol_reload"), Some(id), None, 0.0)),
            SimEvent::BlinkStart { id, .. } => posts.push((cue("blink_start"), Some(id), None, 0.0)),
            SimEvent::BlinkEnd { id } => posts.push((cue("blink_end"), Some(id), None, 0.0)),
            SimEvent::BlinkFizzle { id } => posts.push((cue("power_empty"), Some(id), None, 0.0)),
            SimEvent::WindblastCast { id } => posts.push((cue("windblast"), Some(id), None, 0.0)),
            SimEvent::DarkVision { id, active } if Some(id) == me => {
                posts.push((cue(if active { "dark_vision_on" } else { "dark_vision_off" }), Some(id), None, 0.0))
            }
            SimEvent::Jumped { id } => posts.push((cue("jump"), Some(id), None, 0.0)),
            SimEvent::Mantle { id, edge_height } => {
                let k = if edge_height < 80.0 { "mantle_low" } else if edge_height < 140.0 { "mantle_medium" } else { "mantle_high" };
                posts.push((cue(k), Some(id), None, 0.0));
            }
            SimEvent::Landed { id, speed, .. } if speed > 250.0 => {
                if let Some(p) = player(&view, id) {
                    let s = surface_at(&session, p.position - Vec3::Z * 60.0);
                    posts.push((cues::landing_event(s, speed > 900.0), Some(id), None, 0.0));
                }
            }
            SimEvent::Damage { victim, cause, amount, .. } => {
                let k = match cause {
                    DamageCause::Sword | DamageCause::SwordCounter => Some("sword_hit_body"),
                    DamageCause::Assassination => Some("assassinate"),
                    DamageCause::DropAssassination => Some("assassinate_drop"),
                    DamageCause::Pistol => Some("bullet_body"),
                    DamageCause::PistolHeadshot => Some("bullet_head"),
                    DamageCause::Crossbow => Some("bolt_body"),
                    DamageCause::CrossbowHeadshot => Some("bolt_head"),
                    _ => None,
                };
                if let Some(k) = k {
                    posts.push((cue(k), Some(victim), None, 0.0));
                }
                let human = player(&view, victim).is_none_or(|p| p.kind == PawnKind::Human);
                let vo = if !human { "npc_pain" } else if amount >= 40.0 { "pain_big" } else { "pain_low" };
                posts.push((cue(vo), Some(victim), None, 0.0));
            }
            SimEvent::Death { victim, .. } => {
                posts.push((cue("death_vo"), Some(victim), None, 0.0));
                posts.push((cue("body_fall"), Some(victim), None, -4.0));
            }
            SimEvent::ProjectileImpact { projectile, point, victim: None } => {
                let bolt = a.projectiles.remove(&projectile).unwrap_or(false);
                let s = surface_near(&session, point);
                posts.push((cues::impact_event(bolt, s), None, Some(point), 0.0));
            }
            SimEvent::ProjectileImpact { projectile, .. } => {
                a.projectiles.remove(&projectile);
            }
            _ => {}
        }
        for (ev, subject, point, extra) in posts {
            if ev.is_empty() {
                continue;
            }
            let local = subject.is_some() && subject == me;
            let (anchor, at) = match (subject, point) {
                (_, Some(p)) => (Some(Anchor::Point(p)), p),
                (Some(id), None) => (Some(Anchor::Player(id)), player(&view, id).map(|p| p.position).unwrap_or_default()),
                (None, None) => (None, Vec3::ZERO),
            };
            let anchor = if local { None } else { anchor };
            let ev = if a.cues.contains_key(&ev) { ev } else { fallback_event(&ev) };
            a.post(&mut commands, &mut assets, &ev, anchor, at, extra);
        }
    }
}

/// Surface-specific events that do not exist for every surface fall back to stone.
fn fallback_event(ev: &str) -> String {
    for s in cues::SURFACES {
        if ev.contains(&format!("_{s}_")) || ev.ends_with(&format!("_{s}")) {
            return ev.replacen(s, "Stone", 1);
        }
    }
    ev.to_string()
}

/// Footsteps from movement (the sim emits no per-step events): one step per stride
/// of horizontal travel on the ground.
fn footsteps(
    mut commands: Commands,
    mut audio: ResMut<GameAudio>,
    mut assets: ResMut<Assets<PcmClip>>,
    view: Res<SessionView>,
    session: Res<Session>,
) {
    if audio.lib.is_none() {
        return;
    }
    let me = session.local_id();
    let players: Vec<PlayerState> = view.me.iter().chain(view.others.iter()).copied().collect();
    for p in players {
        let entry = audio.steps.entry(p.id).or_insert((p.position, 0.0));
        let delta = (p.position - entry.0).truncate().length();
        entry.0 = p.position;
        if !p.alive || p.mode != MoveMode::Walking || delta > 200.0 {
            entry.1 = 0.0;
            continue;
        }
        entry.1 += delta;
        let speed = p.velocity.truncate().length();
        let gait = if p.crouched { Gait::Sneak } else if speed > 450.0 { Gait::Sprint } else { Gait::Run };
        let stride = match gait {
            Gait::Sneak => 90.0,
            Gait::Run => 140.0,
            Gait::Sprint => 190.0,
        };
        if entry.1 < stride || speed < 40.0 {
            continue;
        }
        entry.1 -= stride;
        let surface = surface_at(&session, p.position - Vec3::Z * 60.0);
        let npc = p.kind != PawnKind::Human;
        let mut ev = cues::footstep_event(surface, gait, npc);
        if !audio.cues.contains_key(&ev) {
            ev = cues::footstep_event("Stone", gait, npc);
        }
        let local = Some(p.id) == me;
        let anchor = if local { None } else { Some(Anchor::Player(p.id)) };
        audio.post(&mut commands, &mut assets, &ev, anchor, p.position, 0.0);
    }
}

fn listener_uu(cams: &Query<&GlobalTransform, With<SpatialListener>>) -> Option<Vec3> {
    cams.iter().next().map(|t| {
        let b = t.translation() / coords::UU_TO_BEVY;
        Vec3::new(b.x, b.z, b.y)
    })
}

/// Keep ambience slots playing: re-pick when a pick ends; cull far emitters.
fn ambience(
    mut commands: Commands,
    mut audio: ResMut<GameAudio>,
    mut assets: ResMut<Assets<PcmClip>>,
    time: Res<Time>,
    live: Query<(Entity, &AmbientVoice)>,
    cams: Query<&GlobalTransform, With<SpatialListener>>,
) {
    if audio.lib.is_none() || audio.ambience.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    let listener = listener_uu(&cams);
    let mut alive = vec![false; audio.ambience.len()];
    for (e, v) in &live {
        let slot = &audio.ambience[v.0];
        let far = match (slot.pos, listener) {
            (Some(p), Some(l)) => p.distance(l) > slot.range + EMITTER_MARGIN * 2.0,
            _ => false,
        };
        if far {
            commands.entity(e).despawn();
        } else {
            alive[v.0] = true;
        }
    }
    // Finished decodes: start the voices (handles are not cached, so the PCM is freed
    // when the pick ends).
    let done: Vec<AmbientPick> = audio.amb_rx.as_ref().map(|rx| rx.lock().unwrap().try_iter().collect()).unwrap_or_default();
    for (i, items) in done {
        audio.pending[i] = false;
        // Commands apply at the end of the frame; do not re-pick before the voice exists.
        audio.ambience[i].next_try = now + 0.5;
        let (pos, gain, music) = (audio.ambience[i].pos, audio.ambience[i].gain_db, audio.ambience[i].music);
        for (l, pcm) in items {
            let h = assets.add(PcmClip(pcm));
            spawn_voice(&mut commands, h, &l, pos.map(Anchor::Point), pos.unwrap_or_default(), gain, Some(i), if music { 1 } else { 0 });
        }
    }
    let Some(lib) = audio.lib.clone() else { return };
    let Some(tx) = audio.amb_tx.clone() else { return };
    for i in 0..audio.ambience.len() {
        if alive[i] || audio.pending[i] || now < audio.ambience[i].next_try {
            continue;
        }
        let (pos, range) = (audio.ambience[i].pos, audio.ambience[i].range);
        if let (Some(p), Some(l)) = (pos, listener) {
            if p.distance(l) > range + EMITTER_MARGIN {
                audio.ambience[i].next_try = now + 1.0;
                continue;
            }
        }
        let cue = audio.ambience[i].cue.clone();
        let mut leaves = Vec::new();
        let mut r = || audio.rand();
        cue.pick(&mut r, &mut leaves);
        audio.pending[i] = true;
        let (lib, tx) = (lib.clone(), tx.clone());
        std::thread::spawn(move || {
            let items: Vec<(Leaf, Arc<Pcm>)> = leaves.iter().filter_map(|l| Some((*l, Arc::new(lib.decode_uncached(l.media).ok()?)))).collect();
            let _ = tx.send((i, items));
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_voice(commands: &mut Commands, h: Handle<PcmClip>, l: &Leaf, anchor: Option<Anchor>, at: Vec3, extra_db: f32, ambient: Option<usize>, bus: u8) {
    let speed = 2f32.powf(l.pitch_cents / 1200.0).clamp(0.25, 4.0);
    let settings = PlaybackSettings {
        mode: PlaybackMode::Despawn,
        // 3D voices start silent; update_voices sets the attenuated gain on the first frame.
        volume: Volume::Linear(if anchor.is_none() { db(l.volume_db + extra_db) } else { 0.0 }),
        speed,
        spatial: anchor.is_some(),
        spatial_scale: Some(SpatialScale::new(SPATIAL_SCALE)),
        ..PlaybackSettings::ONCE
    };
    let mut e = commands.spawn((
        AudioPlayer(h),
        settings,
        Transform::from_translation(coords::pos(at)),
        Voice { anchor, gain_db: l.volume_db + extra_db, attenuation: l.attenuation, max_distance: l.max_distance, bus },
    ));
    if let Some(i) = ambient {
        e.insert(AmbientVoice(i));
    }
}

/// Follow anchors and apply distance attenuation + master volume.
fn update_voices(
    settings: Res<AudioSettings>,
    audio: Res<GameAudio>,
    view: Res<SessionView>,
    cams: Query<&GlobalTransform, With<SpatialListener>>,
    mut spatial: Query<(&Voice, &mut Transform, &mut SpatialAudioSink), Without<SpatialListener>>,
    mut flat: Query<(&Voice, &mut AudioSink)>,
) {
    let Some(lib) = audio.lib.as_ref() else { return };
    let listener = listener_uu(&cams);
    for (v, mut tf, mut sink) in &mut spatial {
        let pos = match v.anchor {
            Some(Anchor::Player(id)) => player(&view, id).map(|p| p.position),
            Some(Anchor::Point(p)) => Some(p),
            None => None,
        };
        let Some(pos) = pos else { continue };
        tf.translation = coords::pos(pos);
        let d = listener.map(|l| l.distance(pos)).unwrap_or(0.0);
        let att_db = match (v.attenuation, v.max_distance) {
            (_, Some(m)) if d > m => -200.0,
            (Some(a), _) => lib.attenuation_db(a, d),
            _ => {
                let t = (1.0 - d / FALLBACK_RANGE).max(0.0);
                if t <= 0.0 { -200.0 } else { 40.0 * t.log10() }
            }
        };
        sink.set_volume(Volume::Linear(db(v.gain_db + att_db) * settings.volume * bus_gain(&settings, v.bus)));
    }
    for (v, mut sink) in &mut flat {
        sink.set_volume(Volume::Linear(db(v.gain_db) * settings.volume * bus_gain(&settings, v.bus)));
    }
}

/// Periodic one-line summary (events seen, posts, live voices) for logs and the harness.
fn stats(mut audio: ResMut<GameAudio>, time: Res<Time>, voices: Query<(), With<Voice>>) {
    let now = time.elapsed_secs();
    if audio.lib.is_none() || now - audio.last_stats < 10.0 {
        return;
    }
    audio.last_stats = now;
    info!("audio: {} sim events, {} posts, {} live voices", audio.events_seen, audio.posted, voices.iter().count());
}

/// Master volume for every sound of the process: silent when `DISHONORED_MUTE` is set (test runs
/// that must not be heard; sounds still decode and play, so audio checks keep working).
pub fn global_volume() -> GlobalVolume {
    if std::env::var_os("DISHONORED_MUTE").is_some() {
        GlobalVolume::new(bevy::audio::Volume::Linear(0.0))
    } else {
        GlobalVolume::default()
    }
}
