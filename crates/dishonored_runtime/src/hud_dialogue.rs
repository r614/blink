//! Conversation subtitles and player choices on the HUD movie (`UI_HUD_SF` "HUD"), plus the
//! original VO of each line (see `crate::dialogue_vo`).
//!
//! - `subtitles_mc` (class `Subtitles`): `SetSubtitles(name, text | null)`, the line fading in
//!   and out over 0.25 s (`Strong.easeOut`) in its own field (`$NormalFont` 24 px, #e8f7db,
//!   centred, letter spacing 0.5, three lines). The movie has no `_NPC_Name_txt`, so the
//!   speaker's name is not shown. Which line shows is the original's choice (below).
//! - `playerChoice_mc` (class `PlayerChoice`, a `GenericMenu`): `SetMenu(options, title)`
//!   builds `hud_plC_button`s bottom-up (`_offsetBtnY` 2.5), opens them one every 50 ms
//!   (`Back.easeOut` 0.35 s to `_btnOutProps`), highlights the selection (`_btnOverProps`,
//!   0.25 s); confirming plays `valid` and `Validation` and reports the choice 150 ms later;
//!   closing folds the buttons one every 50 ms. Opening gives the movie the focus
//!   (`bAllowFocus`, 0x00B9F390; closing 0x00B8D260 takes it back), so its `InputsManager`
//!   gets the keys: Up / Down move the selection (`Stop` at the ends), Enter confirms; the
//!   number keys are its "DockBar" input, which the menu never takes. The mouse selects the
//!   button it enters (`btn.onRollOver`) and confirms on a click (`onRelease`). On PC the
//!   selected button's A glyph (`_a_mc`) shows only while `_global.bUsingGamepad`
//!   ([`crate::hud_widgets::choice_a_icon_visible`]).
//!
//! Lines come from `MissionView.dialogue` and choices from `MissionView.choices` (shared with
//! the travel-door confirmations), so online clients and replays show what the server decided.

use crate::session::SessionView;
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyboardInput;
use bevy::prelude::*;
use dishonored_sim::input::buttons;

pub struct DialoguePlugin;

impl Plugin for DialoguePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (update_dialogue_hud.before(crate::gfx_hud::update_gfx_hud), crate::dialogue_vo::play_dialogue_vo, crate::dialogue_vo::update_dialogue_voices));
        crate::dialogue_vo::init(app);
    }
}

use dishonored_mission::runtime::{DialogueLineView, MissionView};

/// Where a line's speaker stands: its pawn in the view (the player's own lines at the player);
/// None for unbound speakers (2D voices).
pub fn speaker_pos(view: &SessionView, mv: &MissionView, l: &DialogueLineView) -> Option<Vec3> {
    if l.player {
        return view.me.as_ref().map(|p| p.position);
    }
    let id = l.npc.as_ref().and_then(|n| mv.npc_ids.iter().find(|x| &x.0 == n)).map(|x| x.1)?;
    view.others.iter().chain(view.me.iter()).find(|x| x.id == id).map(|x| x.position)
}

/// The dominant hook (`DisConvGlobalMan.m_CurDominantHook`, 0x00ca71c0 / 0x00c99f80): among
/// the lines of running dialog hooks, the highest dominance above 1e-8 (the first strictly
/// greater wins), dominance `1 - d / (m_fDominantMaxDistance_Percent * m_fVoiceRadius)` within
/// that distance, 0 beyond. Distance: straight line from the local player (APPROXIMATION of the
/// audio path distance `FUN_00c96920`); every NPC speaker is taken as distance-weighted (its
/// interface flag 0x8; the flags 0x2 / 0x10 of other speaker kinds are not decoded). Unplaced
/// speakers count as at the listener.
pub fn dominant_instance(view: &SessionView, mv: &MissionView) -> Option<u32> {
    let listener = view.me.as_ref().map(|p| p.position)?;
    let mut best: (f32, Option<u32>) = (1e-8, None);
    for l in mv.dialogue.iter().filter(|l| l.hook.is_some() && l.instance != 0) {
        let d = speaker_pos(view, mv, l).map(|p| p.distance(listener)).unwrap_or(0.0);
        let r = mv.dialogue_mix.dominant_pct * l.voice_radius;
        let dom = if r > 0.0 && d <= r { 1.0 - d / r } else { 0.0 };
        if dom > best.0 {
            best = (dom, Some(l.instance));
        }
    }
    best.1
}

/// The subtitle to show (`DisConvGlobalMan.m_SubtitlesInfo`, 0x00c9e660, R7): within the main
/// lines (`m_bMainSubtitle`), the best `1 - d / m_fVoiceRadius` above 0 (lines beyond their
/// voice radius are not shown), the dominant hook's line scoring 99; the secondary list
/// (barks, `m_bMainSubtitle` false) picks the same way and shows when no main line does (the
/// HUD movie has one subtitle field). Lines of a dead speaker are already dropped by the
/// server. A voice radius of 0 (an older server) counts as at the speaker.
pub fn current_line<'a>(view: &SessionView, mv: &'a MissionView) -> Option<&'a DialogueLineView> {
    let listener = view.me.as_ref().map(|p| p.position);
    let dominant = dominant_instance(view, mv);
    let score = |l: &DialogueLineView| -> f32 {
        if l.instance != 0 && dominant == Some(l.instance) {
            return 99.0;
        }
        match (listener, speaker_pos(view, mv, l)) {
            (Some(a), Some(b)) if l.voice_radius > 0.0 => 1.0 - a.distance(b) / l.voice_radius,
            _ => 1.0,
        }
    };
    let pick = |main: bool| {
        let mut best: Option<(f32, &'a DialogueLineView)> = None;
        for l in mv.dialogue.iter().filter(|l| l.main == main && !l.text.trim().is_empty()) {
            let s = score(l);
            if s > 0.0 && best.is_none_or(|b| s > b.0) {
                best = Some((s, l));
            }
        }
        best.map(|b| b.1)
    };
    pick(true).or_else(|| pick(false))
}

/// The mouse as the choice menu last saw it.
#[derive(Default)]
pub struct ChoiceMouse {
    /// Last cursor position (window pixels).
    at: Option<Vec2>,
    /// The mission's choice last given to the menu (a harness menu stays until it changes).
    shown: Option<Vec<String>>,
}

/// Feed the subtitle and the choice menu to their HUD widgets.
#[allow(clippy::too_many_arguments)]
pub fn update_dialogue_hud(
    view: Res<SessionView>,
    mut typed: MessageReader<KeyboardInput>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut local: ResMut<crate::input_map::LocalInput>,
    mut hud: ResMut<crate::gfx_hud::GfxHud>,
    mut last: Local<ChoiceMouse>,
) {
    let Some(m) = hud.hud.as_mut() else { return };
    let (st, w) = (&mut m.stage, &mut m.w);
    let mv = view.matchv.mission.as_ref();
    let me = view.me.as_ref().map(|p| p.id);
    let choice: Option<&Vec<String>> = mv.and_then(|m| m.choices.iter().find(|c| Some(c.0) == me).map(|c| &c.1));
    // Subtitles: hidden while a choice is open (the choice replaces the line). The subtitles
    // option (`PSI_Audio_SubtitlesMode`, unset -1 in the original) is not applied: the port's
    // settings cannot tell "unset" from "Off".
    let line = mv.and_then(|m| current_line(&view, m)).filter(|_| choice.is_none()).map(|l| l.text.trim().to_string());
    if let Some(s) = w.subtitles.as_mut() {
        s.set_subtitles(st, line.as_deref());
    }
    let keys: Vec<KeyboardInput> = typed.read().cloned().collect();
    let Some(c) = w.choice.as_mut() else { return };
    let opts: Option<Vec<String>> = choice.map(|o| o.iter().take(4).map(|s| s.trim().to_string()).collect());
    if opts != last.shown {
        c.set_menu(st, &mut w.sounds, opts.as_deref(), None);
        last.shown = opts;
    }
    // `OnPlayerChoiceConfirm(sel._curSelection)` (ExternalInterface, 150 ms after the press):
    // the choice goes to the simulation as the CHOICE button of that index.
    if let Some(i) = c.confirmed.take()
        && let Some(b) = buttons::CHOICES.get(i)
    {
        local.latched |= b;
    }
    // While the choice is open (`SetMovieCanReceiveFocus`, 0x00B9F390) the HUD movie takes the
    // keyboard through its `InputsManager` (`choice_key`) and the mouse over its buttons.
    if c.options.is_none() || !local.enabled || local.override_input.is_some() {
        last.at = None;
        return;
    }
    for k in keys.iter().filter(|k| k.state == ButtonState::Pressed) {
        if let Some(ck) = crate::hud_widgets::choice_key(k.key_code, k.repeat) {
            c.key(st, &mut w.sounds, ck);
        }
    }
    // The engine's mouse position is the window cursor's (FViewport::GetMousePos), hidden and
    // confined to the window while the game has the mouse.
    let cursor = windows.iter().next().and_then(|win| win.cursor_position().map(|p| (p, Vec2::new(win.width(), win.height()))));
    let at = cursor.map(|c| c.0);
    let moved = at.is_some() && at != last.at;
    last.at = at;
    let stage = cursor.map(|(p, size)| {
        let (s, off) = crate::gfx_stage::stage_xform(size);
        (p - off) / s
    });
    c.mouse(st, &mut w.sounds, stage, moved, mouse.just_pressed(MouseButton::Left), mouse.just_released(MouseButton::Left));
}
