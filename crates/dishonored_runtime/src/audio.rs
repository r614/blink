//! Game audio through the original Wwise data, played by `dishonored_audio::engine` (the
//! bank semantics: containers, loops, transitions, states, RTPCs, attenuation curves) and its
//! interactive music. Presentation only: nothing here feeds back into the simulation.
//!
//! - Loading (worker thread): the player/combat banks, the map's ambience and music banks,
//!   the banks of the map's placed `AkAmbientSound` emitters, the contact system (footsteps
//!   and impacts by physical material), the AnimSet sound notifies, the map's music tweaks.
//!   Short sounds are decoded up front; long media decode on the engine's worker threads.
//! - Game objects: every pawn is a Wwise game object at its position; impacts get a
//!   temporary object; placed emitters keep theirs; 2D sounds use the listener object.
//!   The listener is the game camera.
//! - Simulation events, footfalls (contact system, surface from the collision triangle's
//!   physical material), AnimNotify_AkEvent / footfall notifies reported by the animation
//!   code ([`AnimSounds`]), mission sounds (`MissionView::sounds`, posted by Kismet) and the
//!   music manager (`dishonored_audio::music_manager`, driven by the replicated NPC awareness).
//! - On travel the next map's banks load and its ambience replaces the old one.
//!
//! Flags: `--no-audio`, `--volume V` (linear master gain, default 1); env `DISHONORED_MUTE`
//! silences every sound of the process (test runs).

use crate::coords;
use crate::session::{Session, SessionView};
use bevy::audio::{AddAudioSource, AudioPlayer, Decodable, PlaybackSettings, SpatialListener};
use bevy::prelude::*;
use dishonored_audio::anim_notify::{ClipNotifies, NotifyKind};
use dishonored_audio::contact::ContactSystem;
use dishonored_audio::cues::{self, Gait};
use dishonored_audio::engine::{Category, Engine, GLOBAL_OBJ, ObjId};
use dishonored_audio::library::{MediaKey, SoundLibrary};
use dishonored_audio::music_manager::{MusicManager, MusicNpc, MusicTweaks};
use dishonored_audio::Pcm;
use dishonored_audio::volumes::{AudioVolume, volume_at};
use dishonored_sim::events::{DamageCause, SimEvent};
use dishonored_sim::state::{MoveMode, PawnKind, PlayerId, PlayerState};
use dishonored_world::WorldQuery;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

/// Output rate of the engine stream.
const RATE: u32 = 48000;
const PLAYER_OBJ: ObjId = 1 << 32;
const EMITTER_OBJ: ObjId = 2 << 32;
const TEMP_OBJ: ObjId = 3 << 32;

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

/// Decoded PCM as a bevy_audio source (dialogue VO).
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

/// The engine's stereo output as an endless bevy_audio source.
#[derive(Asset, TypePath, Clone)]
pub struct EngineStream(pub Arc<Mutex<Engine>>);

pub struct EngineDecoder {
    engine: Arc<Mutex<Engine>>,
    buf: Vec<f32>,
    pos: usize,
}

impl Iterator for EngineDecoder {
    type Item = bevy::audio::Sample;
    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.buf.len() {
            if let Ok(mut e) = self.engine.lock() {
                e.render(&mut self.buf);
            } else {
                self.buf.iter_mut().for_each(|s| *s = 0.0);
            }
            self.pos = 0;
        }
        let s = self.buf[self.pos];
        self.pos += 1;
        Some(s.clamp(-1.0, 1.0))
    }
}

impl bevy::audio::Source for EngineDecoder {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> bevy::audio::ChannelCount {
        std::num::NonZero::new(2).unwrap()
    }
    fn sample_rate(&self) -> bevy::audio::SampleRate {
        std::num::NonZero::new(RATE).unwrap()
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

impl Decodable for EngineStream {
    type Decoder = EngineDecoder;
    fn decoder(&self) -> EngineDecoder {
        EngineDecoder { engine: self.0.clone(), buf: vec![0.0; 1024 * 2], pos: usize::MAX }
    }
}

/// Clip windows reported by the animation code: the audio side posts the clip's
/// `AnimNotify_AkEvent`s and footfalls that fall inside (original timing, U-031).
#[derive(Resource, Default)]
pub struct AnimSounds {
    pending: Vec<(PlayerId, String, f32, f32, bool)>,
    /// Footfall notifies the locomotion reported directly (`DishonoredNotify_Footfall`).
    footfalls: Vec<(PlayerId, i32)>,
}

impl AnimSounds {
    /// A clip advanced from `from` to `to` (sequence seconds) on a pawn. Looping clips that
    /// wrapped pass `looped` and their raw times.
    pub fn advance(&mut self, player: PlayerId, clip: &str, from: f32, to: f32, looped: bool) {
        if !clip.is_empty() && to != from {
            self.pending.push((player, clip.to_string(), from, to, looped));
        }
    }

    /// A pawn's foot touched down (foot 0 left, 1 right): the third-person locomotion's planted
    /// feet (`m_bFootOnGround`; NPC loops carry no `DishonoredNotify_Footfall`) or a footfall notify.
    /// Pawns reporting footfalls get no stride-based fallback steps.
    pub fn foot_down(&mut self, player: PlayerId, foot: i32) {
        // Nobody drains them when audio is off.
        if self.footfalls.len() > 256 {
            self.footfalls.clear();
        }
        self.footfalls.push((player, foot));
    }
}

struct Emitter {
    obj: ObjId,
    event: String,
    pos: [f32; 3],
    /// "package:actor name" (matches `MissionView::ambient` from Kismet).
    key: String,
    auto_play: bool,
    /// Last Kismet state applied.
    kismet: Option<bool>,
    /// Playing id while it plays.
    playing: u32,
}

struct Loaded {
    map: String,
    lib: SoundLibrary,
    predecoded: Vec<(MediaKey, Arc<Pcm>)>,
    emitters: Vec<Emitter>,
    contact: Option<ContactSystem>,
    notifies: HashMap<String, ClipNotifies>,
    tweaks: Option<MusicTweaks>,
    volumes: Vec<AudioVolume>,
    report: String,
}

#[derive(Resource, Default)]
pub struct GameAudio {
    rx: Option<Mutex<Receiver<anyhow::Result<Loaded>>>>,
    pub engine: Option<Arc<Mutex<Engine>>>,
    map: String,
    loading_map: Option<String>,
    emitters: Vec<Emitter>,
    contact: Option<ContactSystem>,
    notifies: HashMap<String, ClipNotifies>,
    /// The map's `DishonoredAudioVolume`s (ambience states on entry, environments).
    volumes: Vec<AudioVolume>,
    /// Volume the listener is in (its event was posted on entry).
    listener_volume: Option<usize>,
    pub music: Option<MusicManager>,
    stream_spawned: bool,
    /// Projectile id -> fired as a crossbow bolt.
    projectiles: HashMap<u32, bool>,
    steps: HashMap<PlayerId, (Vec3, f32)>,
    /// Last footfall per (player, foot) for the 0.25 s rule.
    foot_time: HashMap<(PlayerId, i32), f32>,
    /// Players whose animation reported notifies recently (sim fallbacks are skipped).
    notify_driven: HashMap<PlayerId, f32>,
    /// Pawns whose animation reported footfalls recently (no stride fallback).
    footfall_driven: HashMap<PlayerId, f32>,
    /// Last post of a notify event per pawn (first- and third-person rigs of the same pawn
    /// may both report a clip).
    notify_recent: HashMap<(PlayerId, String), f32>,
    temp: Vec<(ObjId, f32)>,
    next_temp: u64,
    known_players: HashSet<PlayerId>,
    mission_sound: u32,
    pub posted: u64,
    pub notify_posts: u64,
    pub footfalls: u64,
    events_seen: u64,
    last_stats: f32,
}

pub struct GameAudioPlugin;

impl Plugin for GameAudioPlugin {
    fn build(&self, app: &mut App) {
        let settings = AudioSettings::from_args();
        app.insert_resource(settings);
        app.init_resource::<AnimSounds>();
        if !settings.enabled {
            return;
        }
        app.add_audio_source::<PcmClip>()
            .add_audio_source::<EngineStream>()
            .init_resource::<GameAudio>()
            .add_systems(Update, (sync_volumes, finish_loading, attach_listener, drive, stats).chain());
    }
}

fn map_arg() -> String {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == "--map").and_then(|i| a.get(i + 1)).cloned().unwrap_or_else(|| dishonored_content::DEFAULT_MAP.to_string())
}

fn start_load(audio: &mut GameAudio, map: String) {
    let (tx, rx) = channel();
    audio.loading_map = Some(map.clone());
    std::thread::Builder::new()
        .name("audio-load".into())
        .spawn(move || {
            let _ = tx.send(load(&map));
        })
        .ok();
    audio.rx = Some(Mutex::new(rx));
}

/// Worker thread: banks, emitters, contact system, notifies, music tweaks, short media.
fn load(map: &str) -> anyhow::Result<Loaded> {
    let t0 = std::time::Instant::now();
    let content = dishonored_assets::Content::locate()?;
    let mut lib = SoundLibrary::new(&content.root)?;
    let mut banks: Vec<String> = cues::PLAYER_BANKS.iter().map(|s| s.to_string()).collect();
    let amb = cues::map_ambience(map);
    if let Some(a) = &amb {
        banks.extend(a.banks.iter().map(|s| s.to_string()));
    }
    let mut emitters = Vec::new();
    let mut volumes = Vec::new();
    let mut event_bank: HashMap<String, String> = HashMap::new();
    let mut map_pkgs = Vec::new();
    for p in dishonored_assets::map_packages(&content, map) {
        let Ok(pkg) = content.package(&p) else { continue };
        event_bank.extend(dishonored_audio::emitters::ak_event_banks(&pkg));
        volumes.extend(dishonored_audio::volumes::audio_volumes(&pkg));
        for (i, e) in dishonored_audio::emitters::ambient_emitters(&pkg).into_iter().enumerate() {
            let key = format!("{}:{}", pkg.name, e.actor.rsplit('.').next().unwrap_or(""));
            emitters.push(Emitter { obj: EMITTER_OBJ + ((map_pkgs.len() as u64) << 16) + i as u64, event: e.event, pos: e.position, key, auto_play: e.auto_play, kismet: None, playing: 0 });
        }
        map_pkgs.push(pkg);
    }
    for e in &emitters {
        if let Some(b) = event_bank.get(&e.event) {
            banks.push(b.clone());
        }
    }
    // Banks of the sounds Kismet posts (mission sounds), so they resolve when posted.
    banks.extend(event_bank.values().cloned());
    banks.sort();
    banks.dedup();
    for b in &banks {
        if let Err(e) = lib.load_bank(b) {
            debug!("audio: {b}: {e:#}");
        }
    }
    // Contact system and notifies (DishonoredGame / DishonoredGameContent / Startup).
    let game = content.package("DishonoredGame").ok();
    let game_content = content.package("DishonoredGameContent").ok();
    let contact = game.as_ref().and_then(|g| ContactSystem::load(g, &game_content.iter().map(|p| &**p).collect::<Vec<_>>()));
    let mut notifies = HashMap::new();
    if let Ok(startup) = content.package("Startup") {
        notifies = dishonored_audio::anim_notify::read_package(&startup);
    }
    // NPC AnimSets are cooked into the level packages.
    for p in &map_pkgs {
        for (k, v) in dishonored_audio::anim_notify::read_package(p) {
            notifies.entry(k).or_insert(v);
        }
    }
    let tweaks = MusicTweaks::for_packages(game.as_deref(), &map_pkgs.iter().map(|p| &**p).collect::<Vec<_>>());
    // Decode the short media every cue / contact sound may post.
    let mut events: Vec<String> = cues::all_player_events();
    if let Some(c) = &contact {
        events.extend(c.intersections.iter().filter_map(|i| i.sound.as_ref().map(|s| s.event.clone())));
    }
    events.extend(notifies.values().flat_map(|c| c.notifies.iter()).filter_map(|n| match &n.kind {
        NotifyKind::AkEvent { event, .. } => Some(event.clone()),
        _ => None,
    }));
    events.sort();
    events.dedup();
    let mut keys = Vec::new();
    let mut resolved = 0;
    for ev in &events {
        if let Some(c) = lib.resolve(ev) {
            resolved += 1;
            let mut l = Vec::new();
            c.leaves(&mut l);
            keys.extend(l.iter().filter(|l| !l.media.streamed).map(|l| l.media));
        }
    }
    keys.sort_by_key(|k| (k.bank, k.id));
    keys.dedup();
    let mut predecoded = Vec::new();
    let mut secs = 0.0;
    for k in keys {
        // Only short one-shots up front (footsteps, impacts, whooshes); longer media decode
        // on the engine workers when first played.
        if lib.media_info(k).map(|i| i.0 as f32 / i.1.max(1) as f32).unwrap_or(9.0) > 2.0 {
            continue;
        }
        if let Ok(p) = lib.decode_uncached(k) {
            secs += p.duration();
            predecoded.push((k, Arc::new(p)));
        }
    }
    let report = format!(
        "audio: {map}: {} banks, {resolved}/{} posted events resolve, {} emitters, {} audio volumes, contact system {}, {} clips with sound notifies, music tweaks {}, {} short sounds decoded ({secs:.0} s) in {:.1} s",
        lib.banks.len(),
        events.len(),
        emitters.len(),
        volumes.len(),
        contact.as_ref().map(|c| c.intersections.len()).unwrap_or(0),
        notifies.len(),
        tweaks.as_ref().map(|t| t.name.as_str()).unwrap_or("-"),
        predecoded.len(),
        t0.elapsed().as_secs_f32()
    );
    Ok(Loaded { map: map.to_string(), lib, predecoded, emitters, contact, notifies, tweaks, volumes, report })
}

fn finish_loading(mut commands: Commands, mut audio: ResMut<GameAudio>, mut streams: ResMut<Assets<EngineStream>>, session: Option<Res<Session>>) {
    // (Re)load when the active map changes (level travel).
    let want = session.as_ref().map(|s| s.content.map_name.clone()).unwrap_or_else(map_arg);
    if audio.rx.is_none() && audio.loading_map.as_deref() != Some(want.as_str()) && audio.map != want {
        start_load(&mut audio, want);
    }
    let Some(rx) = &audio.rx else { return };
    let msg = rx.lock().unwrap().try_recv();
    let mut l = match msg {
        Ok(Ok(l)) => l,
        Ok(Err(e)) => {
            warn!("audio disabled: {e:#}");
            audio.rx = None;
            return;
        }
        Err(_) => return,
    };
    audio.rx = None;
    audio.loading_map = None;
    info!("{}", l.report);
    let lib = Arc::new(l.lib);
    let engine = match audio.engine.clone() {
        Some(e) => {
            // Travel: the old map's sounds stop; states and game parameters persist (global
            // in Wwise).
            e.lock().unwrap().replace_library(lib.clone());
            e
        }
        None => {
            let mut e = Engine::new(lib.clone(), RATE);
            e.spawn_decoders(2);
            Arc::new(Mutex::new(e))
        }
    };
    {
        let mut e = engine.lock().unwrap();
        for (k, p) in l.predecoded {
            e.store.insert(k, p);
        }
        // Level ambience: state events first, then the bed, the score and the emitters.
        if let Some(a) = cues::map_ambience(&l.map) {
            // States no event of the map sets (U-161): only when nothing set them before
            // (states persist across levels, so a return from the interior keeps its value).
            for (g, s) in a.states {
                if e.states.get(g).is_none_or(|v| *v == 0) {
                    e.set_state(*g, *s);
                }
            }
            for s in a.state_events {
                e.post_event_name(s, GLOBAL_OBJ);
            }
            e.post_event_name(a.bed, GLOBAL_OBJ);
            if let Some(m) = a.music {
                e.post_event_name(m, GLOBAL_OBJ);
            }
        }
        // Environments of the placed emitters (static): the audio volume around each.
        for em in &l.emitters {
            e.set_object_environments(em.obj, &volume_envs(&l.volumes, em.pos));
        }
        for em in l.emitters.iter_mut().filter(|em| em.auto_play) {
            e.set_position(em.obj, em.pos);
            em.playing = e.post_event_name(&em.event, em.obj);
        }
    }
    audio.music = l.tweaks.map(MusicManager::new);
    audio.map = l.map;
    audio.emitters = l.emitters;
    audio.contact = l.contact;
    audio.notifies = l.notifies;
    audio.volumes = l.volumes;
    audio.listener_volume = None;
    audio.known_players.clear();
    audio.engine = Some(engine.clone());
    if !audio.stream_spawned {
        audio.stream_spawned = true;
        let h = streams.add(EngineStream(engine));
        commands.spawn((AudioPlayer(h), PlaybackSettings::LOOP));
    }
}

/// Environment of a game object: the shareset named by the `m_Environment` of the audio
/// volume around it, at full control value (`SetGameObjectEnvironmentsValues`; LIKELY).
fn volume_envs(vols: &[AudioVolume], p: [f32; 3]) -> Vec<(u32, f32)> {
    volume_at(vols, p).and_then(|i| vols[i].environment.as_deref()).map(|n| vec![(dishonored_audio::fnv1_32(n), 1.0)]).unwrap_or_default()
}

fn attach_listener(mut commands: Commands, cams: Query<Entity, (With<crate::players::GameCamera>, Without<SpatialListener>)>) {
    for e in &cams {
        // Ear gap ~ 20 cm in presentation units (1 uu = 2 cm); used by the dialogue voices.
        commands.entity(e).insert(SpatialListener::new(0.2));
    }
}

fn player<'a>(view: &'a SessionView, id: PlayerId) -> Option<&'a PlayerState> {
    view.me.iter().chain(view.others.iter()).find(|p| p.id == id)
}

fn to_uu(t: &GlobalTransform) -> (Vec3, Vec3) {
    let b = t.translation() / coords::UU_TO_BEVY;
    let r = t.right();
    (Vec3::new(b.x, b.z, b.y), Vec3::new(r.x, r.z, r.y))
}

/// Contact type of the surface below a point (the collision triangle's physical material).
fn surface_below(session: &Session, pos: Vec3, depth: f32) -> Option<(String, f32)> {
    let world = &session.content.collision;
    let hit = world.raycast(pos, -Vec3::Z, depth, dishonored_world::channel::ALL)?;
    Some((world.surface_of(hit.tri).unwrap_or("DisContactType_Environment").to_string(), hit.distance))
}

/// Contact type of the nearest surface around an impact point.
fn surface_near(session: &Session, p: Vec3) -> String {
    let world = &session.content.collision;
    let mut best: Option<(f32, u32)> = None;
    for d in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
        if let Some(hit) = world.raycast(p - d * 12.0, d, 36.0, dishonored_world::channel::ALL) {
            if best.is_none_or(|b| hit.distance < b.0) {
                best = Some((hit.distance, hit.tri));
            }
        }
    }
    best.and_then(|b| world.surface_of(b.1)).unwrap_or("DisContactType_Environment").to_string()
}

/// Events the online client already played from its own prediction (skip the
/// authoritative copy for the local player to avoid doubling).
pub fn predicted_locally(e: &SimEvent, me: Option<PlayerId>) -> bool {
    match e {
        SimEvent::Jumped { id } | SimEvent::BlinkStart { id, .. } | SimEvent::SwordSwing { id } | SimEvent::Fired { id, .. } | SimEvent::Landed { id, .. } => Some(*id) == me,
        _ => false,
    }
}

/// Striker contact type of a pawn's footfall (`DisTweaks_Pawn_Body.m_pFootfallContactType`
/// / `_Run`; the player's `_Player` variants by gait).
fn footfall_striker(p: &PlayerState) -> &'static str {
    let speed = p.velocity.truncate().length();
    if p.kind == PawnKind::Human {
        if p.crouched {
            "DisContactType_Footfall_Sneak_Player"
        } else if p.sprinting {
            "DisContactType_Footfall_Sprint_Player"
        } else {
            "DisContactType_Footfall_Run_Player"
        }
    } else if speed > 300.0 {
        "DisContactType_Footfall_Run"
    } else {
        "DisContactType_Footfall"
    }
}

impl GameAudio {
    /// The original contact system table (impact particles for the effects).
    pub fn contact(&self) -> Option<&ContactSystem> {
        self.contact.as_ref()
    }

    fn temp_obj(&mut self, e: &mut Engine, pos: Vec3, now: f32) -> ObjId {
        self.next_temp += 1;
        let o = TEMP_OBJ + self.next_temp;
        e.set_position(o, pos.to_array());
        e.set_object_environments(o, &volume_envs(&self.volumes, pos.to_array()));
        self.temp.push((o, now + 15.0));
        o
    }

    /// Footfall contact under a pawn (original: trace from the foot / pawn down 1.5 x the
    /// cylinder height, contact system striker x surface).
    fn footfall(&mut self, e: &mut Engine, session: &Session, p: &PlayerState) -> bool {
        let Some(contact) = &self.contact else { return false };
        let Some((surface, _)) = surface_below(session, p.position, 87.5 * 1.5 + 10.0) else { return false };
        let striker = footfall_striker(p);
        let Some(snd) = contact.sound(striker, &surface) else { return false };
        // Speed scaling (0x00BC6290): silent at or below the minimum speed, else the
        // `Scale_RTPC` game parameter on the sound's object.
        let Some(scale) = snd.volume_scale(p.velocity.length()) else { return false };
        let obj = PLAYER_OBJ + p.id as u64;
        e.set_rtpc(obj, dishonored_audio::fnv1_32("Scale_RTPC"), scale);
        let pid = e.post_event_name(&snd.event, obj);
        self.footfalls += 1;
        self.posted += 1;
        pid != 0
    }

    fn post(&mut self, e: &mut Engine, event: &str, obj: ObjId) {
        if !event.is_empty() && e.post_event_name(event, obj) != 0 {
            self.posted += 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn drive(
    mut audio: ResMut<GameAudio>,
    settings: Res<AudioSettings>,
    mut view: ResMut<SessionView>,
    session: Res<Session>,
    time: Res<Time>,
    mut anim: ResMut<AnimSounds>,
    cams: Query<&GlobalTransform, With<crate::players::GameCamera>>,
) {
    let events = std::mem::take(&mut view.audio_events);
    audio.events_seen += events.len() as u64;
    let Some(engine) = audio.engine.clone() else {
        anim.pending.clear();
        anim.footfalls.clear();
        return;
    };
    let mut e = engine.lock().unwrap();
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    e.master = settings.volume;
    e.category_gain = [settings.sfx, settings.music, settings.voice];
    if let Some(cam) = cams.iter().next() {
        let (pos, right) = to_uu(cam);
        e.set_listener(pos.to_array(), right.to_array());
        // Entering an audio volume posts its event (ambience states such as
        // `Set_State_Amb_LVL_Boyle_Int_Streets`) and sets its game parameters.
        let at = volume_at(&audio.volumes, pos.to_array());
        if at != audio.listener_volume {
            audio.listener_volume = at;
            if let Some(v) = at.map(|i| audio.volumes[i].clone()) {
                if let Some(ev) = &v.event {
                    if e.post_event_name(ev, GLOBAL_OBJ) != 0 {
                        audio.posted += 1;
                    }
                }
                for (name, value, _) in &v.rtpcs {
                    e.set_rtpc(GLOBAL_OBJ, dishonored_audio::fnv1_32(name), *value);
                }
                debug!("audio: listener in {} ({:?}, {:?})", v.actor, v.event, v.environment);
            }
        }
    }
    // Pawns are game objects at their positions.
    let me = session.local_id();
    let players: Vec<PlayerState> = view.me.iter().chain(view.others.iter()).copied().collect();
    let mut seen = HashSet::new();
    for p in &players {
        // Sounds come from the pawn's centre (the original posts on the pawn; notifies at bones).
        e.set_position(PLAYER_OBJ + p.id as u64, p.position.to_array());
        let envs = volume_envs(&audio.volumes, p.position.to_array());
        e.set_object_environments(PLAYER_OBJ + p.id as u64, &envs);
        seen.insert(p.id);
    }
    // `TimeDilation` (bus pitch curves): the world's rate of time; under Bend Time every pawn
    // but the caster runs at the world dilation.
    let dilation = players.iter().map(|p| p.time_scale).fold(1.0f32, f32::min).clamp(0.0, 1.0);
    e.set_rtpc(GLOBAL_OBJ, dishonored_audio::fnv1_32(dishonored_audio::bus::TIME_DILATION), dilation);
    let gone: Vec<PlayerId> = audio.known_players.difference(&seen).copied().collect();
    for id in gone {
        e.remove_object(PLAYER_OBJ + id as u64, 200.0);
    }
    audio.known_players = seen;
    let temp_done: Vec<ObjId> = audio.temp.iter().filter(|t| t.1 < now).map(|t| t.0).collect();
    for o in temp_done {
        e.remove_object(o, 0.0);
    }
    audio.temp.retain(|t| t.1 >= now);
    audio.notify_driven.retain(|_, t| now - *t < 2.0);
    audio.footfall_driven.retain(|_, t| now - *t < 2.0);
    audio.notify_recent.retain(|_, t| now - *t < 1.0);

    // Animation notifies.
    for (pid, clip, from, to, looped) in std::mem::take(&mut anim.pending) {
        let Some(c) = audio.notifies.get(&clip).cloned() else { continue };
        let (from, to) = if looped && c.length > 0.0 { (from.rem_euclid(c.length), to.rem_euclid(c.length)) } else { (from, to) };
        for n in c.window(from, to) {
            audio.notify_driven.insert(pid, now);
            match &n.kind {
                NotifyKind::AkEvent { event, .. } => {
                    let key = (pid, event.clone());
                    if audio.notify_recent.get(&key).is_some_and(|t| now - *t < 0.05) {
                        continue;
                    }
                    audio.notify_recent.insert(key, now);
                    if e.post_event_name(event, PLAYER_OBJ + pid as u64) != 0 {
                        audio.notify_posts += 1;
                        audio.posted += 1;
                    }
                }
                NotifyKind::Footfall { foot } => {
                    audio.footfall_driven.insert(pid, now);
                    let last = audio.foot_time.get(&(pid, *foot)).copied().unwrap_or(-1.0);
                    if now - last >= 0.25 {
                        audio.foot_time.insert((pid, *foot), now);
                        if let Some(p) = players.iter().find(|p| p.id == pid) {
                            audio.footfall(&mut e, &session, p);
                        }
                    }
                }
            }
        }
    }

    // Footfall notifies reported directly by the third-person locomotion (0.25 s per foot).
    for (pid, foot) in std::mem::take(&mut anim.footfalls) {
        audio.footfall_driven.insert(pid, now);
        let last = audio.foot_time.get(&(pid, foot)).copied().unwrap_or(-1.0);
        if now - last >= 0.25 {
            audio.foot_time.insert((pid, foot), now);
            if let Some(p) = players.iter().find(|p| p.id == pid) {
                audio.footfall(&mut e, &session, p);
            }
        }
    }

    // Simulation events.
    for ev in events {
        let subject_obj = |id: PlayerId| PLAYER_OBJ + id as u64;
        let cue = |k: &str| cues::cue_event(k).unwrap_or("").to_string();
        let mut posts: Vec<(String, ObjId)> = Vec::new();
        match ev {
            SimEvent::SwordSwing { id } if !audio.notify_driven.contains_key(&id) => posts.push((cue("sword_swing"), subject_obj(id))),
            SimEvent::SwordBlocked { defender, .. } => posts.push((cue("sword_clash"), subject_obj(defender))),
            SimEvent::SwordParried { defender, .. } => {
                posts.push((cue("sword_clash"), subject_obj(defender)));
                posts.push((cue("sword_parry_whoosh"), subject_obj(defender)));
            }
            SimEvent::Fired { id, bolt, projectile } => {
                audio.projectiles.insert(projectile, bolt);
                posts.push((cue(if bolt { "crossbow_fire" } else { "pistol_fire" }), subject_obj(id)));
            }
            SimEvent::Reloaded { id } => posts.push((cue("pistol_reload"), subject_obj(id))),
            SimEvent::BlinkStart { id, .. } => posts.push((cue("blink_start"), subject_obj(id))),
            SimEvent::BlinkEnd { id } => posts.push((cue("blink_end"), subject_obj(id))),
            SimEvent::BlinkFizzle { id } => posts.push((cue("power_empty"), subject_obj(id))),
            SimEvent::WindblastCast { id } => posts.push((cue("windblast"), subject_obj(id))),
            SimEvent::DarkVision { id, active } if Some(id) == me => posts.push((cue(if active { "dark_vision_on" } else { "dark_vision_off" }), subject_obj(id))),
            SimEvent::Jumped { id } => posts.push((cue("jump"), subject_obj(id))),
            SimEvent::Mantle { id, edge_height } => {
                let k = if edge_height < 80.0 { "mantle_low" } else if edge_height < 140.0 { "mantle_medium" } else { "mantle_high" };
                posts.push((cue(k), subject_obj(id)));
            }
            SimEvent::Landed { id, speed, .. } if speed > 250.0 => {
                if let Some(p) = player(&view, id) {
                    // JumpLand contact: big above the hard landing speed, stealth when crouched.
                    let striker = if speed > 900.0 { "DisContactType_JumpLand_Big" } else if p.crouched { "DisContactType_JumpLand_Stealth" } else { "DisContactType_JumpLand" };
                    let surface = surface_below(&session, p.position, 200.0).map(|s| s.0).unwrap_or_else(|| "DisContactType_Environment".into());
                    match audio.contact.as_ref().and_then(|c| c.sound(striker, &surface)) {
                        Some(s) => posts.push((s.event, subject_obj(id))),
                        None => posts.push((cues::landing_event("Stone", speed > 900.0), subject_obj(id))),
                    }
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
                    posts.push((cue(k), subject_obj(victim)));
                }
                let human = player(&view, victim).is_none_or(|p| p.kind == PawnKind::Human);
                let vo = if !human { "npc_pain" } else if amount >= 40.0 { "pain_big" } else { "pain_low" };
                posts.push((cue(vo), subject_obj(victim)));
            }
            // Shadow Kill: `Twk_Pawn_DefaultNPC.m_pShadowKillSound` on the body.
            SimEvent::BodyRemoved { body, ash: true } => posts.push((cue("shadow_kill"), subject_obj(body))),
            SimEvent::Death { victim, .. } => {
                posts.push((cue("death_vo"), subject_obj(victim)));
                posts.push((cue("body_fall"), subject_obj(victim)));
            }
            SimEvent::ProjectileImpact { projectile, point, victim: None } => {
                let bolt = audio.projectiles.remove(&projectile).unwrap_or(false);
                let surface = surface_near(&session, point);
                let striker = if bolt { "DisContactType_Arrow" } else { "DisContactType_Bullet" };
                let obj = audio.temp_obj(&mut e, point, now);
                match audio.contact.as_ref().and_then(|c| c.sound(striker, &surface)) {
                    Some(s) => posts.push((s.event, obj)),
                    None => posts.push((cues::impact_event(bolt, "Stone"), obj)),
                }
            }
            SimEvent::ProjectileImpact { projectile, .. } => {
                audio.projectiles.remove(&projectile);
            }
            _ => {}
        }
        for (ev, obj) in posts {
            audio.post(&mut e, &ev, obj);
        }
    }

    // Footfalls for pawns whose animation does not report notifies: one per stride.
    for p in &players {
        let driven = audio.footfall_driven.contains_key(&p.id);
        let entry = audio.steps.entry(p.id).or_insert((p.position, 0.0));
        let delta = (p.position - entry.0).truncate().length();
        entry.0 = p.position;
        if driven || !p.alive || p.mode != MoveMode::Walking || delta > 200.0 {
            entry.1 = 0.0;
            continue;
        }
        entry.1 += delta;
        let speed = p.velocity.truncate().length();
        let gait = if p.crouched { Gait::Sneak } else if p.sprinting || speed > 450.0 { Gait::Sprint } else { Gait::Run };
        let stride = match gait {
            Gait::Sneak => 90.0,
            Gait::Run => 140.0,
            Gait::Sprint => 190.0,
        };
        if entry.1 < stride || speed < 40.0 {
            continue;
        }
        entry.1 -= stride;
        audio.footfall(&mut e, &session, p);
    }

    // Mission sounds (Kismet `SeqAct_AkPostEvent`, mission actions): 2D on the listener.
    if let Some(mv) = view.matchv.mission.as_ref() {
        let last = audio.mission_sound;
        let new: Vec<(u32, String)> = mv.sounds.iter().filter(|s| s.0 > last).cloned().collect();
        for (id, ev) in new {
            audio.mission_sound = audio.mission_sound.max(id);
            audio.post(&mut e, &ev, GLOBAL_OBJ);
        }
    }

    // Ambient sound actors started, stopped or toggled by Kismet.
    if let Some(mv) = view.matchv.mission.as_ref() {
        for (key, on) in &mv.ambient {
            let Some(i) = audio.emitters.iter().position(|em| em.key.eq_ignore_ascii_case(key)) else { continue };
            if audio.emitters[i].kismet == Some(*on) {
                continue;
            }
            audio.emitters[i].kismet = Some(*on);
            let (obj, playing) = (audio.emitters[i].obj, audio.emitters[i].playing);
            if *on && (playing == 0 || !e.is_playing(playing)) {
                let ev = audio.emitters[i].event.clone();
                e.set_position(obj, audio.emitters[i].pos);
                audio.emitters[i].playing = e.post_event_name(&ev, obj);
            } else if !*on && playing != 0 {
                e.stop_playing(playing, 500.0);
                audio.emitters[i].playing = 0;
            }
        }
    }
    // Music manager from the replicated NPC awareness around the local player.
    if let (Some(m), Some(me_p)) = (audio.music.as_mut(), view.me.as_ref()) {
        let npcs: Vec<MusicNpc> = view
            .matchv
            .ai
            .iter()
            .filter_map(|a| {
                let p = view.others.iter().find(|p| p.id == a.id)?;
                Some(MusicNpc { pos: p.position.to_array(), alive: p.alive, hostile: a.hostile, combat: a.combat, music_type: a.music_type })
            })
            .collect();
        for ev in m.tick(dt, me_p.position.to_array(), &npcs) {
            if e.post_event_name(&ev, GLOBAL_OBJ) != 0 {
                audio.posted += 1;
            }
            info!("music: {ev}");
        }
    }
    let _ = Category::Music;
}

/// Periodic one-line summary (events seen, posts, live voices) for logs and the harness.
fn stats(mut audio: ResMut<GameAudio>, time: Res<Time>) {
    let now = time.elapsed_secs();
    if audio.engine.is_none() || now - audio.last_stats < 10.0 {
        return;
    }
    audio.last_stats = now;
    let (voices, st) = {
        let e = audio.engine.as_ref().unwrap().lock().unwrap();
        (e.active_voices(), e.stats.clone())
    };
    let music = audio.music.as_ref().map(|m| format!("{:?}", m.state)).unwrap_or_default();
    info!(
        "audio: {} sim events, {} posts ({} notify, {} footfalls), {voices} live voices, {} music segments, {} music transitions, music state {music}",
        audio.events_seen, audio.posted, audio.notify_posts, audio.footfalls, st.music_segments, st.music_transitions
    );
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
