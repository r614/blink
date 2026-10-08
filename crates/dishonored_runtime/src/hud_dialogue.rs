//! Conversation subtitles and player choices in the slots of the original HUD movie
//! (`UI_HUD_SF` `HUD`), plus the original VO of each line (see `crate::dialogue_vo`).
//!
//! Layout read from the movie at startup:
//! - `subtitles_mc` (class `Subtitles`, stage (640, 665)) holds one text field `_txt`, centred,
//!   `$NormalFont` 24 px, colour #e8f7db, letter spacing 0.5. `Subtitles.SetSubtitles` also
//!   references a `_NPC_Name_txt` field that this movie does not place, so the speaker's
//!   name is not shown (like the shipped HUD).
//! - `playerChoice_mc` (class `PlayerChoice`, a `GenericMenu`: vertical, bottom- and
//!   left-aligned, stage (473, 665)) with a title `txt`; its options are `hud_plC_button`
//!   instances (text `_txt_mc` at (4.9, -14.35) of each button). The button spacing
//!   (`_offsetBtnY`) is set by ActionScript, which is not run: 34 px is an APPROXIMATION.
//!   The PC movie selects with the mouse; here number keys 1-4 pick (shown as a prefix).
//!
//! Lines come from `MissionView.dialogue` and choices from `MissionView.choices` (shared
//! with the travel-door confirmations), so online clients and replays show what the server
//! decided.

use crate::gfx_hud::GameFonts;
use crate::session::SessionView;
use bevy::prelude::*;
use dishonored_assets::swf::{Movie, Rect as SwfRect};
use ue3_package::ObjRef;
use ue3_package::properties::find;

const STAGE: Vec2 = Vec2::new(1280.0, 720.0);
const MAX_OPTIONS: usize = 4;

pub struct DialoguePlugin;

impl Plugin for DialoguePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_dialogue_hud).add_systems(Update, (update_dialogue_hud, crate::dialogue_vo::play_dialogue_vo, crate::dialogue_vo::update_dialogue_voices));
        crate::dialogue_vo::init(app);
    }
}

/// Stage-space layout of the two slots.
#[derive(Resource, Clone, Copy)]
pub struct DialogueLayout {
    /// Subtitle field: top-left and size (stage px).
    sub_pos: Vec2,
    sub_size: Vec2,
    /// Choice title field and the menu origin (bottom-left of the last option).
    title_pos: Vec2,
    title_width: f32,
    menu_origin: Vec2,
    option_text: Vec2,
    option_width: f32,
    option_step: f32,
}

impl Default for DialogueLayout {
    fn default() -> Self {
        // Values read from the shipped movie (used when it cannot be read).
        Self {
            sub_pos: Vec2::new(351.0, 580.4),
            sub_size: Vec2::new(578.0, 84.0),
            title_pos: Vec2::new(107.0, 549.5),
            title_width: 740.0,
            menu_origin: Vec2::new(473.0, 665.0),
            option_text: Vec2::new(4.9, -14.35),
            option_width: 700.0,
            option_step: 34.0,
        }
    }
}

#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum DialogueText {
    Subtitle,
    Title,
    Option(usize),
}

fn movie_raw(pkg: &ue3_package::Package, name: &str) -> Option<Vec<u8>> {
    let i = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SwfMovie" && pkg.obj_path(ObjRef::Export(i)).ends_with(name))?;
    let h = pkg.read_object(i).ok()?;
    let (n, raw) = find(&h.properties, "RawData").and_then(|p| p.array_raw())?;
    Some(raw[..n].to_vec())
}

fn twips(r: &SwfRect) -> (Vec2, Vec2) {
    (Vec2::new(r.xmin, r.ymin) / 20.0, Vec2::new(r.xmax - r.xmin, r.ymax - r.ymin) / 20.0)
}

/// Read the slot positions from the HUD movie.
fn read_layout() -> Option<DialogueLayout> {
    let content = dishonored_assets::Content::locate().ok()?;
    let pkg = content.package("UI_HUD_SF").ok()?;
    let movie = Movie::parse(&movie_raw(&pkg, "HUD")?).ok()?;
    let root = movie.display_list(&movie.root, 0);
    let at = |name: &str| root.iter().find(|p| p.name.as_deref() == Some(name)).cloned();
    let child = |sprite: u16, name: &str| movie.sprites.get(&sprite).and_then(|s| movie.display_list(s, 0).into_iter().find(|p| p.name.as_deref() == Some(name)));
    let mut l = DialogueLayout::default();
    let sub = at("subtitles_mc")?;
    let sm = sub.matrix.unwrap_or_default();
    if let Some(t) = child(sub.character?, "_txt") {
        let tm = sm.mul(&t.matrix.unwrap_or_default());
        let (off, size) = movie.texts.get(&t.character?).map(twips).unwrap_or((Vec2::ZERO, l.sub_size));
        l.sub_pos = Vec2::new(tm.tx, tm.ty) / 20.0 + off;
        l.sub_size = size;
    }
    let pc = at("playerChoice_mc")?;
    let pm = pc.matrix.unwrap_or_default();
    l.menu_origin = Vec2::new(pm.tx, pm.ty) / 20.0;
    if let Some(title) = child(pc.character?, "_title_mc") {
        let tmm = pm.mul(&title.matrix.unwrap_or_default());
        if let Some(txt) = child(title.character?, "txt") {
            let m = tmm.mul(&txt.matrix.unwrap_or_default());
            let (off, size) = movie.texts.get(&txt.character?).map(twips).unwrap_or((Vec2::ZERO, Vec2::new(l.title_width, 30.0)));
            l.title_pos = Vec2::new(m.tx, m.ty) / 20.0 + off;
            l.title_width = size.x;
        }
    }
    if let Some(btn) = movie.exports.get("hud_plC_button") {
        if let Some(t) = child(*btn, "_txt_mc") {
            let m = t.matrix.unwrap_or_default();
            l.option_text = Vec2::new(m.tx, m.ty) / 20.0;
            let inner = movie.sprites.get(&t.character?).and_then(|s| movie.display_list(s, 0).into_iter().find_map(|p| movie.texts.get(&p.character?).copied()));
            if let Some(r) = inner {
                let (off, size) = twips(&r);
                l.option_text += off;
                l.option_width = size.x.max(200.0);
                l.option_step = (size.y + 6.0).clamp(26.0, 48.0);
            }
        }
    }
    Some(l)
}

pub fn setup_dialogue_hud(mut commands: Commands) {
    let layout = read_layout().unwrap_or_else(|| {
        warn!("dialogue HUD: HUD movie not readable, using built-in slot positions");
        DialogueLayout::default()
    });
    info!("dialogue HUD: subtitles at {:?} size {:?}, choices at {:?} step {}", layout.sub_pos, layout.sub_size, layout.menu_origin, layout.option_step);
    let root = commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, GlobalZIndex(2), Pickable::IGNORE)).id();
    let shadow = TextShadow { offset: Vec2::new(1.5, 1.5), color: Color::srgba(0.0, 0.0, 0.0, 0.85) };
    let spawn = |commands: &mut Commands, kind: DialogueText, justify: Justify, color: Color| {
        commands.spawn((
            Text::new(""),
            TextFont { font_size: FontSize::Px(24.0), ..default() },
            TextColor(color),
            TextLayout::justify(justify),
            shadow,
            Node { position_type: PositionType::Absolute, ..default() },
            Visibility::Hidden,
            kind,
            ChildOf(root),
        ));
    };
    // #e8f7db (subtitles), #e1f0d4 (menu text).
    spawn(&mut commands, DialogueText::Subtitle, Justify::Center, Color::srgb_u8(0xe8, 0xf7, 0xdb));
    spawn(&mut commands, DialogueText::Title, Justify::Left, Color::srgb_u8(0xe1, 0xf0, 0xd4));
    for i in 0..MAX_OPTIONS {
        spawn(&mut commands, DialogueText::Option(i), Justify::Left, Color::srgb_u8(0xe1, 0xf0, 0xd4));
    }
    commands.insert_resource(layout);
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
fn current_line<'a>(view: &SessionView, mv: &'a MissionView) -> Option<&'a DialogueLineView> {
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

pub fn update_dialogue_hud(
    view: Res<SessionView>,
    layout: Option<Res<DialogueLayout>>,
    fonts: Option<Res<GameFonts>>,
    windows: Query<&Window>,
    mut texts: Query<(&DialogueText, &mut Node, &mut Text, &mut TextFont, &mut Visibility)>,
) {
    let Some(layout) = layout else { return };
    let Some(window) = windows.iter().next() else { return };
    let s = (window.width() / STAGE.x).min(window.height() / STAGE.y);
    let off = (Vec2::new(window.width(), window.height()) - STAGE * s) * 0.5;
    let mv = view.matchv.mission.as_ref();
    let me = view.me.as_ref().map(|p| p.id);
    let choice: Option<&Vec<String>> = mv.and_then(|m| m.choices.iter().find(|c| Some(c.0) == me).map(|c| &c.1));
    let title = mv.and_then(|m| m.choice.as_ref()).filter(|c| choice.is_some_and(|o| *o == c.options)).map(|_| String::new());
    let line = mv.and_then(|m| current_line(&view, m)).filter(|_| choice.is_none());
    let n = choice.map(|c| c.len().min(MAX_OPTIONS)).unwrap_or(0);
    for (kind, mut node, mut text, mut font, mut vis) in &mut texts {
        // UI components are written only on change (see gfx_hud::place).
        if let Some(f) = &fonts {
            let want: bevy::text::FontSource = f.normal.clone().into();
            if font.font != want {
                font.font = want;
            }
        }
        let (content, pos, width) = match kind {
            DialogueText::Subtitle => (line.map(|l| l.text.trim().to_string()).unwrap_or_default(), layout.sub_pos, layout.sub_size.x.max(560.0)),
            DialogueText::Title => (title.clone().unwrap_or_default(), layout.title_pos, layout.title_width),
            DialogueText::Option(i) => {
                let content = choice.and_then(|c| c.get(*i)).filter(|_| *i < n).map(|o| format!("{}   {}", i + 1, o.trim())).unwrap_or_default();
                // Bottom-aligned vertical menu: the last option sits on the menu origin.
                let below = n.saturating_sub(1).saturating_sub(*i);
                let y = layout.menu_origin.y - below as f32 * layout.option_step;
                (content, Vec2::new(layout.menu_origin.x, y) + layout.option_text, layout.option_width)
            }
        };
        vis.set_if_neq(if content.is_empty() { Visibility::Hidden } else { Visibility::Inherited });
        crate::gfx_hud::set_text(&mut text, content);
        crate::gfx_hud::set_font_size(&mut font, 24.0 * s);
        let p = pos * s + off;
        crate::gfx_hud::place(&mut node, px(p.x), px(p.y), Some(px(width * s)), None);
    }
}
