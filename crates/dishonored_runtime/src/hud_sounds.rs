//! The HUD's sounds: the names its classes give `SoundHandler.PlaySound`, bound by the HUD's
//! sound theme (`Twk_InGameUI.Twk_GFxMoviePlayerHUD.DisUISoundTheme_0`, plus the common theme
//! `Twk_InGameUI.UISoundTheme_Common`) to `Snd_UI_Ingame` events in `Bank_UI` (the objective
//! window's in `Bank_UI_Objective`), and the power wheel's (`hud_wheel.rs`).

use crate::audio::PcmClip;
use bevy::audio::{AudioPlayer, PlaybackMode, PlaybackSettings, Volume};
use bevy::prelude::*;
use dishonored_audio::Pcm;
use dishonored_audio::library::{Leaf, SoundLibrary};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

/// Theme name -> Wwise event (`AkSoundEventBindings` of `DisUISoundTheme_0`; `Validation` from
/// the common theme).
pub const THEME: &[(&str, &str)] = &[
    ("obj_windowOpen", "UI_Objective_Window"),
    ("obj_taskCompleted", "UI_Objective_Task_Succes"),
    ("obj_taskFailed", "UI_Objective_Task_Fail"),
    ("obj_taskUpdated", "UI_Objective_New_Task"),
    ("obj_taskAdded", "UI_Objective_New_Task"),
    ("H_targetAssassinated", "UI_H_targetAssassinated"),
    ("H_targetRescued", "UI_H_targetRescued"),
    ("H_targetNeutralized", "UI_H_targetNeutralized"),
    ("H_quickPotion", "UI_H_quickPotion"),
    ("H_windowNotification", "UI_H_windowNotification"),
    ("Validation", "UI_Validation"),
    ("Move", "UI_Move"),
    // The power wheel (`Twk_GFxMoviePlayerPowerWheel`: `m_pShowSound` / `m_pHideSound` and
    // its `DisUISoundTheme_0`), in `Bank_UI_Ingame_Wheel`.
    ("W_show", "Snd_UI_Ingame_Wheel_Hand_In"),
    ("W_hide", "Snd_UI_Ingame_Wheel_Hand_Out"),
    ("itemSelection", "Snd_UI_Ingame_Wheel_Select"),
    ("itemValidation", "Snd_UI_Ingame_Wheel_Validation"),
    ("P_AssignShortcut", "UI_P_AssignShortcut"),
    ("P_UnassignShortcut", "UI_P_UnassignShortcut"),
    // `m_pAwarenessStingerEvent` (`Musics.Music_Player_Detected`, `Bank_Music_Player_Detected`).
    ("MusicPlayerDetected", "Music_Player_Detected"),
];

type Decoded = HashMap<String, Vec<(Leaf, Arc<Pcm>)>>;

#[derive(Resource, Default)]
pub struct HudSounds {
    rx: Option<Mutex<Receiver<Decoded>>>,
    sounds: HashMap<String, Vec<(Handle<PcmClip>, f32)>>,
    queue: Vec<String>,
    /// Names played (harness evidence and tests).
    pub played: Vec<String>,
}

impl HudSounds {
    pub fn play(&mut self, name: &str) {
        if self.queue.len() < 16 {
            self.queue.push(name.to_string());
        }
    }
}

#[derive(Component)]
pub struct HudSfx;

pub struct HudSoundsPlugin;

impl Plugin for HudSoundsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HudSounds>().add_systems(Startup, start_loading).add_systems(Update, (finish_loading, play_queue).chain());
    }
}

fn start_loading(mut s: ResMut<HudSounds>) {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("hud-audio".into())
        .spawn(move || {
            let r = (|| -> anyhow::Result<Decoded> {
                let content = dishonored_assets::Content::locate()?;
                let mut lib = SoundLibrary::new(&content.root)?;
                lib.load_bank("Bank_UI")?;
                // The objective window's events (`RequiredBank` Bank_UI_Objective) and the
                // wheel's (Bank_UI_Ingame_Wheel); a missing bank leaves its sounds silent.
                let _ = lib.load_bank("Bank_UI_Objective");
                let _ = lib.load_bank("Bank_UI_Ingame_Wheel");
                let _ = lib.load_bank("Bank_Music_Player_Detected");
                let mut out = HashMap::new();
                for (name, ev) in THEME {
                    let Some(cue) = lib.resolve(ev) else { continue };
                    let mut leaves = Vec::new();
                    cue.leaves(&mut leaves);
                    let v: Vec<(Leaf, Arc<Pcm>)> = leaves.into_iter().filter_map(|l| Some((l, lib.decode(l.media).ok()?))).collect();
                    out.insert(name.to_string(), v);
                }
                Ok(out)
            })();
            match r {
                Ok(d) => {
                    let _ = tx.send(d);
                }
                Err(e) => eprintln!("hud audio unavailable: {e:#}"),
            }
        })
        .ok();
    s.rx = Some(Mutex::new(rx));
}

fn finish_loading(mut s: ResMut<HudSounds>, mut assets: ResMut<Assets<PcmClip>>) {
    let Some(rx) = &s.rx else { return };
    let Ok(sounds) = rx.lock().unwrap().try_recv() else { return };
    for (k, v) in sounds {
        let hs = v.into_iter().map(|(l, p)| (assets.add(PcmClip(p)), 10f32.powf(l.volume_db / 20.0))).collect();
        s.sounds.insert(k, hs);
    }
    s.rx = None;
}

fn play_queue(mut commands: Commands, mut s: ResMut<HudSounds>, settings: Option<Res<crate::settings::GameSettings>>) {
    if s.queue.is_empty() {
        return;
    }
    let q = std::mem::take(&mut s.queue);
    let g = settings.map(|st| st.s.bus_gain(dishonored_settings::Bus::Sfx)).unwrap_or(1.0);
    for name in q {
        if s.played.len() >= 256 {
            s.played.drain(..128);
        }
        s.played.push(name.clone());
        let Some(hs) = s.sounds.get(&name).cloned() else { continue };
        for (h, v) in hs {
            commands.spawn((AudioPlayer(h), PlaybackSettings { mode: PlaybackMode::Despawn, volume: Volume::Linear(g * v), ..PlaybackSettings::ONCE }, HudSfx));
        }
    }
}
