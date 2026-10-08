//! Original VO for conversation lines: each line in `MissionView.dialogue` carries its Wwise
//! event and voice bank; a worker thread resolves it in `Bk_<GUID>.INT`
//! (`dishonored_audio::vo`) and decodes it, then the clip plays 3D on the speaking NPC's pawn
//! (Wwise attenuation curve of the sound) or 2D for player lines and unbound speakers.
//! Presentation only; lines that end early (interrupted) stop their voice.

use crate::audio::{AudioSettings, PcmClip};
use crate::coords;
use crate::session::SessionView;
use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, PlaybackMode, PlaybackSettings, SpatialAudioSink, SpatialListener, SpatialScale, Volume};
use bevy::prelude::*;
use dishonored_audio::Pcm;
use dishonored_audio::vo::VoResolver;
use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

const SPATIAL_SCALE: f32 = 0.003;

struct Request {
    line: u32,
    event: String,
    bank: String,
    pawn: Option<u32>,
}

struct Ready {
    line: u32,
    pawn: Option<u32>,
    pcm: Arc<Pcm>,
    volume_db: f32,
    /// Dry volume (dB) every 100 uu out to the attenuation's max distance.
    curve: Vec<f32>,
}

#[derive(Resource)]
pub struct DialogueVo {
    tx: Sender<Request>,
    rx: Mutex<Receiver<Ready>>,
    requested: HashSet<u32>,
    pub played: u32,
}

#[derive(Component)]
pub struct DialogueVoice {
    line: u32,
    pawn: Option<u32>,
    gain_db: f32,
    curve: Vec<f32>,
    /// Dominant-hook ducking scale (1 = full), blended toward its target.
    duck: f32,
}

pub fn init(app: &mut App) {
    let enabled = app.world().get_resource::<AudioSettings>().is_some_and(|s| s.enabled);
    if !enabled {
        return;
    }
    let (tx, req_rx) = channel::<Request>();
    let (ready_tx, rx) = channel::<Ready>();
    std::thread::Builder::new()
        .name("dialogue-vo".into())
        .spawn(move || worker(req_rx, ready_tx))
        .ok();
    app.insert_resource(DialogueVo { tx, rx: Mutex::new(rx), requested: HashSet::new(), played: 0 });
}

fn worker(rx: Receiver<Request>, tx: Sender<Ready>) {
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    let Ok(mut vo) = VoResolver::open(&content.root) else {
        warn!("dialogue VO disabled: codebooks not found");
        return;
    };
    while let Ok(r) = rx.recv() {
        let Some(clip) = vo.resolve(&r.event, &r.bank) else {
            debug!("dialogue VO: {} not in {}", r.event, r.bank);
            continue;
        };
        let Ok(pcm) = vo.lib.decode(clip.leaf.media) else { continue };
        let max = clip.leaf.max_distance.unwrap_or(3000.0);
        let curve = (0..=((max / 100.0).ceil() as usize))
            .map(|i| match clip.leaf.attenuation {
                Some(a) => vo.lib.attenuation_db(a, i as f32 * 100.0),
                None => {
                    let t = (1.0 - i as f32 * 100.0 / max).max(0.0);
                    if t <= 0.0 { -200.0 } else { 40.0 * t.log10() }
                }
            })
            .collect();
        let _ = tx.send(Ready { line: r.line, pawn: r.pawn, pcm, volume_db: clip.leaf.volume_db, curve });
    }
}

fn db(v: f32) -> f32 {
    10f32.powf(v / 20.0)
}

/// Request new lines, start decoded ones, stop voices whose line is gone.
pub fn play_dialogue_vo(mut commands: Commands, vo: Option<ResMut<DialogueVo>>, view: Res<SessionView>, clips: Option<ResMut<Assets<PcmClip>>>, voices: Query<(Entity, &DialogueVoice)>) {
    // No clip assets when audio is disabled (--no-audio).
    let (Some(mut vo), Some(mut clips)) = (vo, clips) else { return };
    let Some(mv) = view.matchv.mission.as_ref() else { return };
    let live: HashSet<u32> = mv.dialogue.iter().map(|l| l.id).collect();
    for l in &mv.dialogue {
        if vo.requested.contains(&l.id) || l.remaining < 0.25 {
            continue;
        }
        let (Some(event), Some(bank)) = (l.vo_event.clone(), l.vo_bank.clone()) else { continue };
        vo.requested.insert(l.id);
        let pawn = if l.player { None } else { l.npc.as_ref().and_then(|n| mv.npc_ids.iter().find(|x| &x.0 == n)).map(|x| x.1) };
        let _ = vo.tx.send(Request { line: l.id, event, bank, pawn });
    }
    let ready: Vec<Ready> = vo.rx.lock().unwrap().try_iter().collect();
    for r in ready {
        if !live.contains(&r.line) {
            continue;
        }
        vo.played += 1;
        let spatial = r.pawn.is_some();
        info!("dialogue VO: playing line {} ({:.2}s, {})", r.line, r.pcm.duration(), if spatial { "3D on the speaker" } else { "2D" });
        commands.spawn((
            AudioPlayer(clips.add(PcmClip(r.pcm))),
            PlaybackSettings { mode: PlaybackMode::Despawn, volume: Volume::Linear(if spatial { 0.0 } else { db(r.volume_db) }), spatial, spatial_scale: Some(SpatialScale::new(SPATIAL_SCALE)), ..PlaybackSettings::ONCE },
            Transform::default(),
            DialogueVoice { line: r.line, pawn: r.pawn, gain_db: r.volume_db, curve: r.curve, duck: 1.0 },
        ));
    }
    for (e, v) in &voices {
        if !live.contains(&v.line) {
            commands.entity(e).despawn();
        }
    }
    let n = vo.requested.len();
    if n > 512 {
        let keep: HashSet<u32> = vo.requested.iter().copied().filter(|id| live.contains(id)).collect();
        vo.requested = keep;
    }
}

/// The ducking target of a voice (`FUN_00c96cb0` with `m_CurDominantHook`, R7): while a
/// dialog hook dominates, the voices of the other running hooks play at
/// `m_fNonDominantVolumeScale`; voices of conversations not started by a hook, and every voice
/// while no hook dominates, play at full volume.
fn duck_target(mv: Option<&dishonored_mission::runtime::MissionView>, dominant: Option<u32>, line: u32) -> f32 {
    let (Some(mv), Some(d)) = (mv, dominant) else { return 1.0 };
    match mv.dialogue.iter().find(|l| l.id == line) {
        Some(l) if l.hook.is_some() && l.instance != 0 && l.instance != d => mv.dialogue_mix.non_dominant_scale,
        _ => 1.0,
    }
}

/// Move the ducking scale toward its target at `m_fAudioDuckBlendSpeed` per second (the blend
/// curve of `FUN_00c96cb0` is not decoded: linear in the scale, APPROXIMATION).
fn blend_duck(duck: &mut f32, target: f32, speed: f32, dt: f32) {
    let step = speed * dt;
    *duck = if (*duck - target).abs() <= step { target } else { *duck + step * (target - *duck).signum() };
}

/// Follow the speaking pawn and apply the distance attenuation, the dominant-hook ducking and
/// master volume.
pub fn update_dialogue_voices(
    settings: Option<Res<AudioSettings>>,
    view: Res<SessionView>,
    time: Res<Time>,
    cams: Query<&GlobalTransform, With<SpatialListener>>,
    mut spatial: Query<(&mut DialogueVoice, &mut Transform, &mut SpatialAudioSink)>,
    mut flat: Query<(&mut DialogueVoice, &mut AudioSink), Without<SpatialAudioSink>>,
) {
    let master = settings.map(|s| s.volume).unwrap_or(1.0);
    let mv = view.matchv.mission.as_ref();
    let dominant = mv.and_then(|m| crate::hud_dialogue::dominant_instance(&view, m));
    let speed = mv.map(|m| m.dialogue_mix.blend_speed).unwrap_or(5.0);
    let dt = time.delta_secs();
    let listener = cams.iter().next().map(|t| {
        let b = t.translation() / coords::UU_TO_BEVY;
        Vec3::new(b.x, b.z, b.y)
    });
    for (mut v, mut tf, mut sink) in &mut spatial {
        let target = duck_target(mv, dominant, v.line);
        blend_duck(&mut v.duck, target, speed, dt);
        let Some(p) = v.pawn.and_then(|id| view.others.iter().chain(view.me.iter()).find(|x| x.id == id)) else { continue };
        let at = p.position + Vec3::Z * 60.0;
        tf.translation = coords::pos(at);
        let d = listener.map(|l| l.distance(at)).unwrap_or(0.0);
        let i = (d / 100.0) as usize;
        let att = v.curve.get(i).copied().unwrap_or(-200.0);
        sink.set_volume(Volume::Linear(db(v.gain_db + att) * v.duck * master));
    }
    for (mut v, mut sink) in &mut flat {
        let target = duck_target(mv, dominant, v.line);
        blend_duck(&mut v.duck, target, speed, dt);
        sink.set_volume(Volume::Linear(db(v.gain_db) * v.duck * master));
    }
}
