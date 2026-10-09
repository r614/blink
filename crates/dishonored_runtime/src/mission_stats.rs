//! The end-of-mission statistics screen (`DisGFxMoviePlayerMissionStats`, movie
//! `UI_MissionStats_SF` `MissionStats`, tweak `Twk_UI_MissionStats.Twk_GFxMoviePlayerMissionStats`),
//! rebuilt from the movie's classes and fed the way the native side feeds it:
//!
//! - data: the mission's `DisTweaks_MissionStats` (`Twk_MissionStats.Twk_M*` in
//!   `DishonoredGameFull_P`): `m_MissionStats` rows (description, `EDisPlayerStat`,
//!   `m_bAddOtherStat` + `m_AdditionalStat`, `m_bCheckbox`, `m_bCheckIfNonZero`),
//!   `m_MissionStatsMaxValues`, `m_MissionSummaryItems` (description, story flag, value),
//!   `m_BackgroundImagePath`; the values are the record `DisSeqAct_MissionStatsTracking` input 1
//!   made (`MissionView::stats_record`);
//! - `SetMissionStats` (vslot 120, 0x00BD3CA0) calls `_root.missionStats_mc.SetMissionStats`
//!   with (stats list, pickups list, the mission's name `DishonoredGameInfo.m_MissionNames[n]`,
//!   `img://` + the background path, `m_bOpenedFromMenu`, replay available). Each row
//!   (0x00BCBEA0): `statName` the description; the value rounded `(int)(v * 2 + 0.5) >> 1`;
//!   `ePlayerStat_ChaosLevel` (0x1B) reads `m_HighChaos` / `m_LowChaos`; a checkbox row a boolean;
//!   the pickup stats (0x21 .. 0x25, 0x27) go to the pickups list with `maxValue` and their icon
//!   (0x00BB9BD0); the others carry `sectionName` = `m_MissionStats`. Then each summary item whose
//!   story flag has its `m_bStoryFlagValue` (0x00BCBC00, `sectionName` = `m_MissionSummary`), or
//!   one `m_EmptyMissionSummaryItem` line when none;
//! - `MS_StatsScreen` (`ms_statsScreen`: `_bkgd_mc` at (640, 360), `_title_mc` (117.3, 164.6),
//!   `_list_mc` (138.7, 177.25), `_pickUps_mc` (1185.95, 378.15)): the black veil fades in over
//!   0.75 s, then `OpenElements` (`OpenScreen`, the help, the pickups from +100 / +50 in 1.25 s,
//!   the background and the list open), `OnOpened` 1.3 s later; `A` (`t_ContinueGame`) plays
//!   `Validation` and closes (`OnContinueClicked`), `X` replays (`t_ReplayMission`, the port has
//!   no mission replay: help `MissionsStats_replayLocked`);
//! - `MS_StatsList.SetStatsList`: a `ms_statList_section` header whenever `sectionName` changes
//!   (25 px after the previous clip), rows `ms_statList_item` at x 30, 4 px apart, `sep_mc` frame
//!   cycling 1..4; `SetStat`: a boolean shows `checkBox_mc` (frame 1 checked, 2 not), else
//!   `value_txt`; `Open` shows a row every 40 ms;
//! - `MS_PickUpsList`: `_title_txt` = `t_Found`, `line0_mc` .. `line4_mc` with
//!   `statName <font color="#FFFFFF">value / max</font>` and `ic_mc` at the icon's label;
//! - `MS_Background`: the background image (`ImgLoader`, not resized, centred) at 105 %, the
//!   image from 115 % and 50 px up, Corvo from 120 % (0.9 s).
//!
//! The GFx 3D properties the classes tween (`_xrotation`, `_yrotation`, `_z`) are not drawn
//! (`menu_art.rs`).

use crate::menu_anim::{Act, Anim, Ease, P};
use crate::menu_art::{self, DisplayList, FontKind, LayerCache, MovieArt, Props, States, TEXT, px_matrix};
use crate::menu_audio::MenuSounds;
use crate::menu_ui::{MenuArt, MenuEvent, MenuNav, MenuTexts};
use crate::menu_widgets::{HelpBar, texts_any};
use bevy::prelude::*;
use dishonored_assets::swf::Matrix;
use dishonored_mission::runtime::MissionStatsRecord;
use dishonored_sim::player_stats::{CHAOS_LEVEL, pickup_icon};

/// One `DisMissionStat`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatRowDef {
    pub description: String,
    pub stat: String,
    pub add_other: bool,
    pub additional: String,
    pub checkbox: bool,
    pub check_nonzero: bool,
    /// `m_MissionStatsMaxValues[i]` (0 when absent).
    pub max: i32,
}

/// One `DisMissionSummaryItem`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SummaryDef {
    pub description: String,
    /// `m_StoryFlag` by name (`Startup.upk` story flags).
    pub flag: String,
    pub value: bool,
}

/// One `DisTweaks_MissionStats`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MissionStatsDef {
    /// Object path (`Twk_MissionStats.Twk_M0_Prison`).
    pub path: String,
    pub mission_number: i32,
    pub summary: Vec<SummaryDef>,
    pub rows: Vec<StatRowDef>,
    /// `m_BackgroundImagePath` (`UI_MissionStatsBg_Prison.MissionStats_Prison`).
    pub background: String,
}

/// `Twk_GFxMoviePlayerMissionStats` texts.
#[derive(Clone, Debug, PartialEq)]
pub struct StatsTexts {
    pub mission_summary: String,
    pub empty_summary: String,
    pub mission_stats: String,
    pub high: String,
    pub low: String,
}

impl Default for StatsTexts {
    /// `Twk_UI_MissionStats.int` (INT).
    fn default() -> Self {
        Self { mission_summary: "Special Actions".into(), empty_summary: "No special actions accomplished".into(), mission_stats: "Mission Stats".into(), high: "High".into(), low: "Low".into() }
    }
}

/// What a list row's `statValue` is.
#[derive(Clone, Debug, PartialEq)]
pub enum StatValue {
    /// `value_txt` with a number.
    Number(i32),
    /// `value_txt` with a string (the chaos level).
    Text(String),
    /// `checkBox_mc` (checked or not).
    Check(bool),
    /// `undefined`: an empty `value_txt` (summary lines).
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListRow {
    pub section: String,
    pub name: String,
    pub value: StatValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PickupRow {
    pub name: String,
    pub value: i32,
    pub max: i32,
    pub icon: &'static str,
}

/// The row value rounding of 0x00BD3CA0: `ROUND(v * 2 + 0.5) >> 1` (x87 round to nearest even).
pub fn round_stat(v: f32) -> i32 {
    ((v * 2.0 + 0.5).round_ties_even() as i32) >> 1
}

/// The lists `SetMissionStats` receives (0x00BD3CA0 with 0x00BCBEA0 / 0x00BCBC00).
pub fn screen_rows(def: &MissionStatsDef, rec: &MissionStatsRecord, texts: &StatsTexts) -> (Vec<ListRow>, Vec<PickupRow>) {
    let mut list = Vec::new();
    let mut pickups = Vec::new();
    for r in &def.rows {
        let v = round_stat(rec.row_value(&r.stat, r.add_other.then_some(r.additional.as_str())));
        if let Some(icon) = pickup_icon(&r.stat) {
            pickups.push(PickupRow { name: r.description.clone(), value: v, max: r.max, icon });
            continue;
        }
        let value = if r.stat == CHAOS_LEVEL {
            StatValue::Text(if v != 0 { texts.high.clone() } else { texts.low.clone() })
        } else if r.checkbox {
            StatValue::Check(if r.check_nonzero { v != 0 } else { v == 0 })
        } else {
            StatValue::Number(v)
        };
        list.push(ListRow { section: texts.mission_stats.clone(), name: r.description.clone(), value });
    }
    let mut any = false;
    for s in &def.summary {
        if rec.flag(&s.flag) == s.value {
            list.push(ListRow { section: texts.mission_summary.clone(), name: s.description.clone(), value: StatValue::None });
            any = true;
        }
    }
    if !any {
        list.push(ListRow { section: texts.mission_summary.clone(), name: texts.empty_summary.clone(), value: StatValue::None });
    }
    (list, pickups)
}

/// Read every `DisTweaks_MissionStats` of `DishonoredGameFull_P`.
pub fn load_defs(content: &dishonored_assets::Content) -> Vec<MissionStatsDef> {
    use ue3_package::ObjRef;
    use ue3_package::properties::{Property, Value};
    let Ok(pkg) = content.package("DishonoredGameFull_P") else { return Vec::new() };
    let flags = dishonored_conversation::load::StoryFlags::load(&content.cooked).unwrap_or_default();
    let s = |p: &[Property], n: &str| p.iter().find(|x| x.name == n).and_then(|x| if let Value::Str(s) = &x.value { Some(s.clone()) } else { None }).unwrap_or_default();
    let b = |p: &[Property], n: &str| p.iter().find(|x| x.name == n).is_some_and(|x| matches!(x.value, Value::Bool(true)));
    let e = |p: &[Property], n: &str| p.iter().find(|x| x.name == n).and_then(|x| match &x.value { Value::Enum(s) | Value::Name(s) => Some(s.clone()), Value::Byte(b) => dishonored_sim::player_stats::ALL.get(*b as usize).map(|s| s.to_string()), _ => None });
    let mut out = Vec::new();
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "DisTweaks_MissionStats" {
            continue;
        }
        let Ok(h) = pkg.read_object(i) else { continue };
        let mut d = MissionStatsDef { path: pkg.obj_path(ObjRef::Export(i)), ..Default::default() };
        let mut maxes = Vec::new();
        for p in &h.properties {
            match p.name.as_str() {
                "m_MissionNumber" => {
                    if let Value::Int(n) = p.value {
                        d.mission_number = n;
                    }
                }
                "m_BackgroundImagePath" => {
                    if let Value::Str(v) = &p.value {
                        d.background = v.clone();
                    }
                }
                "m_MissionStatsMaxValues" => maxes = p.array_objects().unwrap_or_default(),
                "m_MissionSummaryItems" => {
                    for st in p.array_structs(&pkg).unwrap_or_default() {
                        let g = st.iter().find(|x| x.name == "m_StoryFlag").and_then(|x| if let Value::Guid(g) = x.value { Some(g) } else { None });
                        d.summary.push(SummaryDef { description: s(&st, "m_Description"), flag: g.map(|g| flags.name(&g)).unwrap_or_default(), value: b(&st, "m_bStoryFlagValue") });
                    }
                }
                "m_MissionStats" => {
                    for st in p.array_structs(&pkg).unwrap_or_default() {
                        d.rows.push(StatRowDef {
                            description: s(&st, "m_Description"),
                            stat: e(&st, "m_Stat").unwrap_or_else(|| dishonored_sim::player_stats::ALL[0].to_string()),
                            add_other: b(&st, "m_bAddOtherStat"),
                            additional: e(&st, "m_AdditionalStat").unwrap_or_else(|| dishonored_sim::player_stats::ALL[0].to_string()),
                            checkbox: b(&st, "m_bCheckbox"),
                            check_nonzero: b(&st, "m_bCheckIfNonZero"),
                            max: 0,
                        });
                    }
                }
                _ => {}
            }
        }
        for (r, m) in d.rows.iter_mut().zip(maxes) {
            r.max = m;
        }
        out.push(d);
    }
    out
}

/// The tweak for a record: its own path, else (no tracking op ran: missions not made of the
/// original Kismet) the first mission's, whose rows every `Twk_M*` shares.
pub fn def_for<'a>(defs: &'a [MissionStatsDef], tweak: &str) -> Option<&'a MissionStatsDef> {
    defs.iter().find(|d| d.path.eq_ignore_ascii_case(tweak)).or_else(|| defs.iter().find(|d| d.mission_number == 0))
}

/// `img://` package and texture of a background path (`UI_MissionStatsBg_Prison.MissionStats_Prison`
/// is the texture `MissionStats_Prison` of `UI_MissionStatsBg_Prison_SF`).
pub fn background_texture(path: &str) -> Option<(String, String)> {
    let (pkg, name) = path.split_once('.')?;
    Some((format!("{pkg}_SF"), name.rsplit('.').next().unwrap_or(name).to_string()))
}

/// `MS_StatsList._offsetOriginX`, `_offsetY`, `_offsetSectionY`.
const ROW_X: f32 = 30.0;
const ROW_GAP: f32 = 4.0;
const SECTION_GAP: f32 = 25.0;
/// `MS_StatsScreen.Open`: the veil's fade before `OpenElements`.
const VEIL_SECS: f64 = 0.75;

#[derive(Resource, Default)]
pub struct MissionStatsScreen {
    pub open: bool,
    root: Option<Entity>,
    cache: LayerCache,
    anim: Anim,
    now: f64,
    opened_at: f64,
    /// `DisSeqAct_ShowMissionStats` count seen, and whether the outcome was shown.
    shown_seen: u32,
    outcome_seen: bool,
    defs: Option<Vec<MissionStatsDef>>,
    texts: StatsTexts,
    title: String,
    list: Vec<ListRow>,
    pickups: Vec<PickupRow>,
    bg: Option<(Handle<Image>, Vec2)>,
    /// The player closed it (`OnContinueClicked`).
    pub closed: bool,
    /// Open on the next frame (harness).
    pub request: bool,
}

pub fn setup(mut commands: Commands) {
    let root = menu_art::spawn_layer(&mut commands, 49);
    commands.entity(root).insert(Visibility::Hidden);
    commands.insert_resource(MissionStatsScreen { root: Some(root), anim: Anim::new(0x57a7), ..default() });
}

/// Load the tweaks, texts and the record's rows, then `Open`.
fn open(s: &mut MissionStatsScreen, mv: &dishonored_mission::runtime::MissionView, t: &MenuTexts, art: Option<&MenuArt>, images: &mut Assets<Image>, sounds: &mut MenuSounds, now: f64) {
    let content = dishonored_assets::Content::locate().ok();
    if s.defs.is_none() {
        s.defs = Some(content.as_ref().map(load_defs).unwrap_or_default());
        if let Some(c) = &content {
            let loc = dishonored_assets::loc::Localization::load(&c.root, "INT", &["Twk_UI_MissionStats"]);
            let sec = "Twk_GFxMoviePlayerMissionStats DisTweaks_GFxMoviePlayerMissionStats";
            let g = |k: &str, d: &str| loc.get(sec, k).map(dishonored_assets::loc::plain).unwrap_or_else(|| d.to_string());
            let d = StatsTexts::default();
            s.texts = StatsTexts { mission_summary: g("m_MissionSummary", &d.mission_summary), empty_summary: g("m_EmptyMissionSummaryItem", &d.empty_summary), mission_stats: g("m_MissionStats", &d.mission_stats), high: g("m_HighChaos", &d.high), low: g("m_LowChaos", &d.low) };
        }
    }
    // Without a record (no tracking op ran) the stats are the live tracked ones.
    let rec = mv.stats_record.clone().unwrap_or_else(|| MissionStatsRecord { tweak: String::new(), stats: mv.player_stats.clone(), chaos_high: mv.high_chaos, flags: Vec::new() });
    let defs = s.defs.clone().unwrap_or_default();
    let def = def_for(&defs, &rec.tweak).cloned().unwrap_or_default();
    let (list, pickups) = screen_rows(&def, &rec, &s.texts);
    s.list = list;
    s.pickups = pickups;
    // `DishonoredGameInfo.m_MissionNames[m_MissionNumber]` (the tweak's mission) or the
    // mission's own title.
    s.title = if rec.tweak.is_empty() { mv.title.clone() } else { t.loc.get("DishonoredGameInfo", &format!("m_MissionNames[{}]", def.mission_number)).map(dishonored_assets::loc::plain).unwrap_or_else(|| mv.title.clone()) };
    s.bg = None;
    if let (Some(c), Some((pkg, name))) = (&content, background_texture(&def.background)) {
        s.bg = menu_art::load_texture(c, &pkg, &name, images);
    }
    s.open = true;
    s.closed = false;
    s.opened_at = now;
    let a = &mut s.anim;
    a.clear();
    // `Open`: the veil 0 -> 100 in 0.75 s; `OpenElements` then.
    a.author("ms/veil", Props { alpha: 0.0, ..Props::default() });
    a.now(now, "ms/veil", Act::To(P::new().alpha(100.0), VEIL_SECS as f32, Ease::StrongOut));
    let t1 = now + VEIL_SECS;
    a.at(t1, "ms/veil", Act::To(P::new().alpha(0.0), 0.3, Ease::StrongOut));
    for k in ["ms/bkgd", "ms/list", "ms/pickups", "ms/title"] {
        a.author(k, Props::default());
        a.now(now, k, Act::Set(P::new().visible(false)));
        a.at(t1, k, Act::Set(P::new().visible(true)));
    }
    // `_pickUps_mc` from alpha 0, +100 / +50 (1.25 s).
    a.at(t1, "ms/pickups", Act::From(P::new().alpha(0.0).x(100.0).y(50.0), 1.25, Ease::StrongOut));
    // `MS_StatsList.Open`: from alpha 0 and 150 px down (1.25 s); a row every 40 ms.
    a.at(t1, "ms/list", Act::From(P::new().alpha(0.0).y(150.0), 1.25, Ease::StrongOut));
    for i in 0..s.list.len() {
        let k = format!("ms/row/{i}");
        a.author(&k, Props::default());
        a.now(now, &k, Act::Set(P::new().visible(false)));
        let ti = t1 + 0.04 * i as f64;
        a.at(ti, &k, Act::Set(P::new().visible(true)));
        a.at(ti, &k, Act::From(P::new().alpha(0.0).x(20.0).scale(150.0), 0.25, Ease::StrongOut));
    }
    // `MS_Background.Open`: the image from 115 % and 50 px up to 105 % (0.9 s).
    a.author("ms/img", Props { xscale: 105.0, yscale: 105.0, ..Props::default() });
    a.at(t1, "ms/img", Act::Set(P::new().y(-50.0).scale(115.0)));
    a.at(t1, "ms/img", Act::To(P::new().y(0.0).scale(105.0), 0.9, Ease::StrongOut));
    if let Some(c) = art.and_then(|a| a.get("stats")) {
        if let Some((ch, m)) = c.export("ms_statsScreen").and_then(|sc| c.child(sc, "_title_mc")) {
            let _ = ch;
            a.author("ms/title", Props::from_matrix(&m, 100.0));
        }
    }
    HelpBar::open(a, "ms/help", t1);
    sounds.play("OpenScreen");
}

#[allow(clippy::too_many_arguments)]
pub fn toggle(
    mut s: ResMut<MissionStatsScreen>,
    mut nav: ResMut<MenuNav>,
    mut local: ResMut<crate::input_map::LocalInput>,
    mut sounds: ResMut<MenuSounds>,
    mut images: ResMut<Assets<Image>>,
    art: Option<Res<MenuArt>>,
    texts: Option<Res<MenuTexts>>,
    time: Res<Time<Real>>,
    mut vis: Query<&mut Visibility, With<menu_art::MenuLayer>>,
    (view, mut pause): (Res<crate::session::SessionView>, ResMut<crate::pause_menu::PauseMenu>),
) {
    let now = time.elapsed_secs_f64();
    s.now = now;
    let mv = view.matchv.mission.as_ref();
    // `DisSeqAct_ShowMissionStats`, or the mission's successful end.
    let mut want = std::mem::take(&mut s.request);
    if let Some(m) = mv {
        if m.stats_shown > s.shown_seen {
            s.shown_seen = m.stats_shown;
            want = true;
        }
        let success = m.outcome.as_ref().is_some_and(|o| o.success);
        if success && !s.outcome_seen {
            s.outcome_seen = true;
            want = true;
        }
        if !success {
            s.outcome_seen = false;
        }
    }
    if want && !s.open {
        if let (Some(m), Some(t)) = (mv, texts.as_deref()) {
            open(&mut s, m, t, art.as_deref(), &mut images, &mut sounds, now);
            nav.enabled = false;
            nav.settle = 1;
            nav.focus = None;
            local.enabled = false;
        }
    }
    if s.open {
        s.anim.advance(now);
        nav.enabled = now - s.opened_at >= VEIL_SECS + 1.3;
        nav.horizontal = false;
        nav.wrap = false;
        pause.overlay |= crate::pause_menu::OVERLAY_STATS;
    } else {
        pause.overlay &= !crate::pause_menu::OVERLAY_STATS;
    }
    if let Some(root) = s.root {
        if let Ok(mut v) = vis.get_mut(root) {
            let want = if s.open { Visibility::Inherited } else { Visibility::Hidden };
            if *v != want {
                *v = want;
            }
        }
    }
}

pub fn handle(mut s: ResMut<MissionStatsScreen>, mut nav: ResMut<MenuNav>, mut sounds: ResMut<MenuSounds>, mut local: ResMut<crate::input_map::LocalInput>) {
    if !s.open {
        return;
    }
    let events = std::mem::take(&mut nav.events);
    for ev in events {
        let cont = match &ev {
            MenuEvent::Activate { id, .. } => id == "help:A" || id == "ms:continue",
            MenuEvent::Button("A") => true,
            _ => false,
        };
        if cont {
            // `APressed`: `Validation`, `OnContinueClicked`, `Close`.
            sounds.play("Validation");
            s.open = false;
            s.closed = true;
            nav.enabled = false;
            nav.items.clear();
            local.enabled = true;
            return;
        }
    }
}

fn build(s: &MissionStatsScreen, art: &MenuArt, t: &MenuTexts, now: f64) -> DisplayList {
    let mut dl = DisplayList::default();
    let Some(ms) = art.get("stats") else { return dl };
    let Some(screen) = ms.export("ms_statsScreen") else { return dl };
    let clock = (now - s.opened_at) as f32;
    // `_bkgd_mc` (`MS_Background`): the image under the frame art.
    if let Some((bch, bm)) = ms.child(screen, "_bkgd_mc") {
        if s.anim.get("ms/bkgd", now).visible {
            if let Some((tex, size)) = &s.bg {
                let ip = s.anim.get("ms/img", now);
                let (cx, cy) = (bm.tx / 20.0, bm.ty / 20.0 + ip.y);
                let (hw, hh) = (size.x * ip.xscale / 200.0, size.y * ip.yscale / 200.0);
                dl.prims.push(menu_art::Prim::Quad { tex: tex.clone(), src: [0.0, 0.0, size.x, size.y], dst: [(cx - hw, cy - hh), (cx + hw, cy - hh), (cx + hw, cy + hh), (cx - hw, cy + hh)], color: [1.0, 1.0, 1.0, 1.0] });
            }
            let hide = |p: &str| if p == "_img_mc" { Some(Props { visible: false, ..Props::default() }) } else { None };
            dl.movie_fx(ms, bch, bm, &States::default().clock(clock), 1.0, &hide, false);
        }
    }
    // `_black_mc` and the frame (the named children are drawn below).
    let skip = |p: &str| match p {
        "_bkgd_mc" | "_title_mc" | "_list_mc" | "_pickUps_mc" => Some(Props { visible: false, ..Props::default() }),
        _ => None,
    };
    dl.movie_fx(ms, screen, Matrix::default(), &States::default(), 1.0, &skip, true);
    // Title (`MS_StatsScreenTitle.SetTitle`: the mission's name).
    if let Some((tch, tm)) = ms.child(screen, "_title_mc") {
        let tp = s.anim.get("ms/title", now);
        if tp.visible {
            dl.movie(ms, tch, tm, &States::default().clock(clock), tp.alpha / 100.0);
            for f in DisplayList::fields(ms, tch, tm, &States::default()) {
                dl.field_text(ms, &f, &s.title.to_uppercase(), None);
            }
        }
    }
    draw_list(&mut dl, s, ms, screen, &art.metrics, now);
    draw_pickups(&mut dl, s, ms, screen, t, now);
    // The black veil (`blackVeil_mc`, `ms_BlackVeil` at (640, 360)).
    let veil = s.anim.get("ms/veil", now);
    if veil.alpha > 0.0 {
        if let Some(v) = ms.export("ms_BlackVeil") {
            dl.movie_fx(ms, v, px_matrix(640.0, 360.0), &States::default(), veil.alpha / 100.0, &|_| None, true);
        }
    }
    if let Some(v) = ms.export("m_Vignette") {
        dl.movie(ms, v, px_matrix(640.0, 360.0), &States::default(), 1.0);
    }
    if let Some(c) = art.get("common") {
        // `MissionsStats_replayLocked`: Continue only (no mission replay in the port).
        HelpBar::draw(&mut dl, c, &art.metrics, t, &s.anim, "ms/help", now, &[("A", "t_ContinueGame")]);
    }
    dl
}

fn draw_list(dl: &mut DisplayList, s: &MissionStatsScreen, ms: &MovieArt, screen: u16, metrics: &menu_art::FontMetrics, now: f64) {
    let Some((lch, lm)) = ms.child(screen, "_list_mc") else { return };
    let lp = s.anim.get("ms/list", now);
    if !lp.visible {
        return;
    }
    let lm = lm.mul(&Props { y: lp.y, ..Props::default() }.matrix());
    let la = lp.alpha / 100.0;
    dl.movie(ms, lch, lm, &States::default(), la);
    let (Some(sec_ch), Some(item_ch)) = (ms.export("ms_statList_section"), ms.export("ms_statList_item")) else { return };
    let sec_h = ms.bounds(sec_ch, Matrix::default(), &States::default()).map(|r| r.height()).unwrap_or(40.0);
    let item_h = ms.bounds(item_ch, Matrix::default(), &States::default()).map(|r| r.height()).unwrap_or(30.0);
    let sep_frames = ms.child(item_ch, "sep_mc").and_then(|(c, _)| ms.sprite(c)).map(|s| s.frames.len()).unwrap_or(4).max(1);
    let mut section = String::new();
    let mut y_next = 0.0f32; // prevMc._y + prevMc._height
    let mut first = true;
    let mut sep = 1usize;
    let _ = metrics;
    for (i, r) in s.list.iter().enumerate() {
        if r.section != section {
            section = r.section.clone();
            let y = if first { 0.0 } else { y_next + SECTION_GAP };
            first = false;
            let m = lm.mul(&px_matrix(0.0, y));
            dl.movie(ms, sec_ch, m, &States::default(), la);
            for f in DisplayList::fields(ms, sec_ch, m, &States::default()) {
                let mut f = f;
                f.color.mul[3] *= la;
                dl.field_text(ms, &f, &section.to_uppercase(), None);
            }
            y_next = y + sec_h;
        }
        let y = y_next + ROW_GAP;
        y_next = y + item_h;
        let rp = s.anim.get(&format!("ms/row/{i}"), now);
        if !rp.visible {
            continue;
        }
        let a = la * rp.alpha / 100.0;
        let m = lm.mul(&px_matrix(ROW_X + rp.x, y));
        // `sep_mc.gotoAndStop(n)`, n cycling 1 .. its frame count; the checkbox `on` / `off`.
        let mut st = States::default().with("sep_mc", &format!("#{}", sep - 1));
        sep = if sep < sep_frames { sep + 1 } else { 1 };
        let check = match r.value {
            StatValue::Check(c) => Some(c),
            _ => None,
        };
        st = st.with("checkBox_mc", if check == Some(true) { "on" } else { "off" });
        let hide = move |p: &str| if p == "checkBox_mc" && check.is_none() { Some(Props { visible: false, ..Props::default() }) } else { None };
        dl.movie_fx(ms, item_ch, m, &st, a, &hide, false);
        for f in DisplayList::fields_fx(ms, item_ch, m, &st, &hide) {
            let text = if f.path.contains("name_txt") {
                r.name.clone()
            } else if f.path.contains("value_txt") {
                match &r.value {
                    StatValue::Number(n) => n.to_string(),
                    StatValue::Text(t) => t.clone(),
                    _ => continue,
                }
            } else {
                continue;
            };
            let mut f = f;
            f.color.mul[3] *= a;
            dl.field_text(ms, &f, &text, None);
        }
    }
}

fn draw_pickups(dl: &mut DisplayList, s: &MissionStatsScreen, ms: &MovieArt, screen: u16, t: &MenuTexts, now: f64) {
    let Some((pch, pm)) = ms.child(screen, "_pickUps_mc") else { return };
    let pp = s.anim.get("ms/pickups", now);
    if !pp.visible {
        return;
    }
    let pm = pm.mul(&Props { x: pp.x, y: pp.y, ..Props::default() }.matrix());
    let a = pp.alpha / 100.0;
    // Lines without a pickup keep their authored (empty) state.
    let n = s.pickups.len();
    let hide = move |p: &str| {
        let i = p.strip_prefix("line").and_then(|r| r.strip_suffix("_mc")).and_then(|d| d.parse::<usize>().ok())?;
        (i >= n).then_some(Props { visible: false, ..Props::default() })
    };
    let mut st = States::default();
    for (i, pk) in s.pickups.iter().enumerate() {
        st = st.with(&format!("line{i}_mc/ic_mc"), pk.icon);
    }
    dl.movie_fx(ms, pch, pm, &st, a, &hide, false);
    for f in DisplayList::fields_fx(ms, pch, pm, &st, &hide) {
        let text = if f.path.contains("_title_txt") {
            texts_any(t, "t_Found").to_uppercase()
        } else if let Some(i) = f.path.strip_prefix("line").and_then(|r| r.split('_').next()).and_then(|d| d.parse::<usize>().ok()) {
            match s.pickups.get(i) {
                // `statName <font color="#FFFFFF">value / max</font>` (right-aligned, autoSize).
                Some(pk) => format!("{} {} / {}", pk.name, pk.value, pk.max),
                None => continue,
            }
        } else {
            continue;
        };
        let mut f = f;
        f.color.mul[3] *= a;
        dl.field_text(ms, &f, &text, None);
    }
    let _ = (FontKind::Normal, TEXT);
}

#[allow(clippy::too_many_arguments)]
pub fn rebuild(mut commands: Commands, mut s: ResMut<MissionStatsScreen>, art: Option<Res<MenuArt>>, t: Option<Res<MenuTexts>>, mut nav: ResMut<MenuNav>, fonts: Option<Res<crate::gfx_hud::GameFonts>>) {
    let (Some(art), Some(t), Some(root)) = (art, t, s.root) else { return };
    if !s.open {
        return;
    }
    let now = s.now;
    let mut dl = build(&s, &art, &t, now);
    dl.item("ms:continue", Rect::new(0.0, 0.0, 0.0, 0.0), true);
    nav.set_items(dl.items.clone());
    let s = &mut *s;
    menu_art::spawn_list_cached(&mut commands, root, &dl, fonts.as_deref(), &mut s.cache);
}
