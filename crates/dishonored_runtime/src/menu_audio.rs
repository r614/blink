//! Menu sound: the original UI sound theme and menu music, decoded from the install.
//!
//! - `Twk_InGameUI.UISoundTheme_Common` (DishonoredGame.upk) binds the movies' sound names to
//!   `Snd_UI_Ingame` events in `Bank_UI`: Move, validation, back, change, ChangeMainTab,
//!   ChangeSubTab, OpenScreen, CloseScreen (and Confirmation, Failure, Reset ...).
//! - `Twk_UI_MainMenu.Twk_GFxMoviePlayerMainMenu` adds `M_PressStart` / `M_OpenMainMenu` and
//!   starts `Musics.Music_Menu` (`Bank_Music_Menu`, a 130 s looping stream).
//!
//! Volumes follow the settings (global x SFX for UI, global x music for music).

use crate::audio::PcmClip;
use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, PlaybackMode, PlaybackSettings, Volume};
use bevy::prelude::*;
use dishonored_audio::Pcm;
use dishonored_audio::library::{Leaf, SoundLibrary};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

/// UI sound names (theme binding names) -> original Wwise event.
pub const THEME: &[(&str, &str)] = &[
    ("OpenScreen", "UI_OpenScreen"),
    ("CloseScreen", "UI_CloseScreen"),
    ("Move", "UI_Move"),
    ("validation", "UI_Validation"),
    ("Confirmation", "UI_Confirmation"),
    ("back", "UI_Back"),
    ("Failure", "UI_Failure"),
    ("change", "UI_Change"),
    ("ChangeMainTab", "UI_ChangeMainTab"),
    ("ChangeSubTab", "UI_ChangeSubTab"),
    ("Reset", "UI_Reset"),
    ("M_PressStart", "UI_M_PressStart"),
    ("M_OpenMainMenu", "UI_M_OpenMainMenu"),
];
pub const MUSIC_EVENT: &str = "Music_Menu";

type Decoded = (HashMap<String, Vec<(Leaf, Arc<Pcm>)>>, Option<Vec<(Leaf, Arc<Pcm>)>>);

#[derive(Resource, Default)]
pub struct MenuSounds {
    rx: Option<Mutex<Receiver<Decoded>>>,
    sounds: HashMap<String, Vec<(Handle<PcmClip>, f32)>>,
    music: Option<Handle<PcmClip>>,
    /// Sounds requested this frame (theme names).
    queue: Vec<String>,
    pub want_music: bool,
    pub loaded: bool,
    /// Count of sounds played (harness evidence).
    pub played: u64,
}

impl MenuSounds {
    pub fn play(&mut self, name: &str) {
        if self.queue.len() < 16 {
            self.queue.push(name.to_string());
        }
    }
}

#[derive(Component)]
pub struct MenuMusic;
#[derive(Component)]
pub struct MenuSfx;

pub struct MenuAudioPlugin {
    /// Register the PCM audio source (false when `GameAudioPlugin` already did).
    pub register_source: bool,
    pub music: bool,
}

impl Plugin for MenuAudioPlugin {
    fn build(&self, app: &mut App) {
        use bevy::audio::AddAudioSource;
        if self.register_source {
            app.add_audio_source::<PcmClip>();
        }
        app.insert_resource(MenuSounds { want_music: self.music, ..default() })
            .add_systems(Startup, start_loading)
            .add_systems(Update, (finish_loading, play_queue, music, volumes).chain());
    }
}

fn start_loading(mut s: ResMut<MenuSounds>) {
    let (tx, rx) = channel();
    let want_music = s.want_music;
    std::thread::Builder::new()
        .name("menu-audio".into())
        .spawn(move || {
            let r = (|| -> anyhow::Result<Decoded> {
                let content = dishonored_assets::Content::locate()?;
                let mut lib = SoundLibrary::new(&content.root)?;
                lib.load_bank("Bank_UI")?;
                if want_music {
                    lib.load_bank("Bank_Music_Menu")?;
                }
                let decode = |ev: &str| -> Option<Vec<(Leaf, Arc<Pcm>)>> {
                    let cue = lib.resolve(ev)?;
                    let mut leaves = Vec::new();
                    cue.leaves(&mut leaves);
                    Some(leaves.into_iter().filter_map(|l| Some((l, lib.decode(l.media).ok()?))).collect())
                };
                let mut out = HashMap::new();
                for (name, ev) in THEME {
                    if let Some(v) = decode(ev) {
                        out.insert(name.to_string(), v);
                    }
                }
                let music = if want_music { decode(MUSIC_EVENT) } else { None };
                Ok((out, music))
            })();
            match r {
                Ok(d) => {
                    let _ = tx.send(d);
                }
                Err(e) => eprintln!("menu audio unavailable: {e:#}"),
            }
        })
        .ok();
    s.rx = Some(Mutex::new(rx));
}

fn finish_loading(mut s: ResMut<MenuSounds>, mut assets: ResMut<Assets<PcmClip>>) {
    let Some(rx) = &s.rx else { return };
    let Ok((sounds, music)) = rx.lock().unwrap().try_recv() else { return };
    for (k, v) in sounds {
        let hs = v.into_iter().map(|(l, p)| (assets.add(PcmClip(p)), 10f32.powf(l.volume_db / 20.0))).collect();
        s.sounds.insert(k, hs);
    }
    s.music = music.and_then(|m| m.into_iter().next()).map(|(_, p)| assets.add(PcmClip(p)));
    s.rx = None;
    s.loaded = true;
    info!("menu audio: {} UI sounds, music {}", s.sounds.len(), s.music.is_some());
}

fn gain(settings: Option<&crate::settings::GameSettings>, bus: dishonored_settings::Bus) -> f32 {
    settings.map(|s| s.s.bus_gain(bus)).unwrap_or(1.0)
}

fn play_queue(mut commands: Commands, mut s: ResMut<MenuSounds>, settings: Option<Res<crate::settings::GameSettings>>) {
    if s.queue.is_empty() {
        return;
    }
    let q = std::mem::take(&mut s.queue);
    let g = gain(settings.as_deref(), dishonored_settings::Bus::Sfx);
    for name in q {
        let Some(hs) = s.sounds.get(&name).cloned() else { continue };
        s.played += 1;
        for (h, v) in hs {
            commands.spawn((AudioPlayer(h), PlaybackSettings { mode: PlaybackMode::Despawn, volume: Volume::Linear(g * v), ..PlaybackSettings::ONCE }, MenuSfx));
        }
    }
}

fn music(mut commands: Commands, s: Res<MenuSounds>, playing: Query<Entity, With<MenuMusic>>, settings: Option<Res<crate::settings::GameSettings>>) {
    let on = s.want_music && s.music.is_some();
    match (on, playing.iter().next()) {
        (true, None) => {
            let g = gain(settings.as_deref(), dishonored_settings::Bus::Music);
            commands.spawn((AudioPlayer(s.music.clone().unwrap()), PlaybackSettings { mode: PlaybackMode::Loop, volume: Volume::Linear(g), ..PlaybackSettings::LOOP }, MenuMusic));
        }
        (false, Some(e)) => commands.entity(e).despawn(),
        _ => {}
    }
}

fn volumes(settings: Option<Res<crate::settings::GameSettings>>, mut music: Query<&mut AudioSink, With<MenuMusic>>) {
    let Some(st) = settings else { return };
    if !st.is_changed() {
        return;
    }
    let g = st.s.bus_gain(dishonored_settings::Bus::Music);
    for mut m in &mut music {
        m.set_volume(Volume::Linear(g));
    }
}
