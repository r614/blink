//! The in-game journal (`DisGFxMoviePlayerJournal`, movie `UI_Journal_SF` `Journal`, tweak
//! `Twk_UI_Journal.Twk_GFxMoviePlayerJournal`), rebuilt from the movie's classes:
//!
//! - opening: `GBA_Journal` (`DefaultInput.ini`: `J` -> `Dis_ToggleJournal`, 0x009EE550 ->
//!   0x00AA3290 -> 0x00BD7940) toggles it; `DisSeqAct_OpenJournal` opens it at `m_Tab`
//!   (`MissionView::journal_open`). The movie's start (0x00BD11E0) sets `StartTabIndex` (the
//!   powers / charms tabs fall back to 0 when unavailable), `PowersTabAvailable`,
//!   `BoneCharmsTabAvailable` and `PlayerCoins`, then fires the player's
//!   `DisSeqEvent_JournalViewed` (0x00BCFAC0 with the class of 0x00BBC040): the client sends
//!   `actions::OPEN_JOURNAL` so the host fires it (`kismet_host::journal_inputs`). The game pauses
//!   while it is up (`bPauseGameWhileActive`), the HUD hides (`m_bShowHUDWhileActive` false);
//! - `JournalBase`: `bkgd_mc` (`p_bkgd`) under the `Journal` clip at (640, 360), the vignette;
//!   `JournalScreen.InitTabs`: tabs in `JournalTabsOrder` (objectives 0, logs 1, powers 2, bone
//!   charms 3, inventory 4), `lib_mTabs_tab_s` at (0, -260) with `_offsetTabX` -60; `Open`:
//!   `_bkgdTabs_mc` from 250 px left (0.3 s); `CloseJournal`: tabs out, content to 105 % and
//!   alpha 0 (0.3 s), `closeJournal` after `_animCloseDuration` 0.75 s;
//! - `JournalTabContent_tabs`: sub tabs (`j_SubTabsBkgd`, `j_SubTabs`) at (-250, -185), the
//!   `SubTab`s left-aligned at x -544 (`JournalSubTabs.SetTabs`) 25 px apart; a tab change slides
//!   the content 250 px (0.25 s, `OpenTabContent`), a sub tab change hides it in 0.15 s;
//! - `j_ItemsListScreen`: the list at (-512, -155), the details at (355, -280);
//! - objectives (`j_ObjectivesScreen` sub tabs Tasks / Mission clues / Mission items): the list
//!   is `Req_ObjectivesList` (0x00BD79B0, see [`objectives_list`]) drawn by `j_ObjectivesList`
//!   (`t_PrimaryTasks` / `t_SecondaryTasks` sections, objectives at x 0, tasks at
//!   `_startOriginTaskX` 55, 20 px after a section, the state icon `ic` at the `objState`
//!   label, `Y` (Alt / Numpad 3) toggles the history); the details are the chapter's title and
//!   briefing (`SetChapterInfos`); the coins (`j_obj_moneyCart_` at (-590, 288),
//!   `t_CoinsCount` / `t_CoinCount`);
//! - logs (`j_LogsScreen` sub tabs Written notes / Books / Audiographs): the held abstract items
//!   whose `m_JournalDisplaySection` is `DJIS_Note` (8) / `DJIS_Book` (12) / `DJIS_AudioLog` (7)
//!   (`Req_Logs_*` -> 0x00BD15E0), their `m_ItemName` in `j_logs_logsList_item` rows and the
//!   selected one's `m_Description` in `j_logs_logDetails`.

use crate::menu_anim::{Act, Anim, Ease, P};
use crate::menu_art::{self, DisplayList, FontKind, FontMetrics, LayerCache, MovieArt, Props, States, TEXT, px_matrix};
use crate::menu_audio::MenuSounds;
use crate::menu_ui::{MenuArt, MenuEvent, MenuNav, MenuTexts, SEC_BASE};
use crate::menu_widgets::{HelpBar, texts_any};
use bevy::prelude::*;
use dishonored_assets::swf::Matrix;
use dishonored_mission::TaskState;
use dishonored_mission::runtime::ObjectiveView;
use std::collections::HashMap;

/// `[DisGFxMoviePlayerJournal_Texts]` (`DishonoredGame.int`).
pub const SEC_JOURNAL: &str = "DisGFxMoviePlayerJournal_Texts";

/// `JournalTabsOrder` (the stats tab, 5, is not built by `InitTabs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Objectives,
    Logs,
    Powers,
    BoneCharms,
    Inventory,
}

impl Tab {
    /// `EDisJournalTab` index.
    pub fn index(self) -> i32 {
        self as i32
    }
    /// The tab label (`[DisGFxMoviePlayerBase_Texts]`) and its content clip (`linkage_ID`).
    pub fn label_key(self) -> &'static str {
        match self {
            Tab::Objectives => "t_Objectives",
            Tab::Logs => "t_Logs",
            Tab::Powers => "t_Powers",
            Tab::BoneCharms => "t_BoneCharms",
            Tab::Inventory => "t_Inventory",
        }
    }
    pub fn linkage(self) -> &'static str {
        match self {
            Tab::Objectives => "j_obj_mc",
            Tab::Logs => "j_logs_mc",
            Tab::Powers => "j_powers_mc",
            Tab::BoneCharms => "j_boneCharms_mc",
            Tab::Inventory => "j_inventory_mc",
        }
    }
    /// Sub tabs (`_tabsContent` of the tab's screen class): text key and the journal sections
    /// their list shows.
    pub fn subs(self) -> &'static [(&'static str, Sub)] {
        match self {
            Tab::Objectives => &[("t_Tasks", Sub::Tasks), ("t_ChapterNotes", Sub::Items(&[9])), ("t_MissionItems", Sub::Items(&[1]))],
            Tab::Logs => &[("t_Notes", Sub::Items(&[8])), ("t_Books", Sub::Items(&[12])), ("t_AudioLogs", Sub::Items(&[7]))],
            // `j_InventoryScreen`: Resources / Key ring / Gadgets / Ammo / Upgrades
            // (`req_Inventory_*`). Their native builders (0x00BD1C70, 0x00BD19E0, 0x00BD1F00,
            // 0x00BD20C0, 0x00BD2340) also list inventory items, ammo and upgrades; the port lists
            // the held abstract items of the matching section.
            Tab::Inventory => &[("t_Resources", Sub::Items(&[3])), ("t_KeyRing", Sub::Items(&[2])), ("t_Gadgets", Sub::Items(&[4])), ("t_Ammo", Sub::Items(&[5])), ("t_Upgrades", Sub::Items(&[6]))],
            Tab::Powers | Tab::BoneCharms => &[],
        }
    }
}

/// What a sub tab lists.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sub {
    /// `Req_ObjectivesList`.
    Tasks,
    /// Held abstract items of these `EDisJournalItemSection`s.
    Items(&'static [u8]),
}

/// The tabs `InitTabs` builds: objectives, logs, powers when available, bone charms when
/// available, inventory (`PowersTabAvailable` / `BoneCharmsTabAvailable` from 0x00BD11E0).
pub fn tabs(powers: bool, charms: bool) -> Vec<Tab> {
    let mut t = vec![Tab::Objectives, Tab::Logs];
    if powers {
        t.push(Tab::Powers);
    }
    if charms {
        t.push(Tab::BoneCharms);
    }
    t.push(Tab::Inventory);
    t
}

/// `StartTabIndex` for an `EDisJournalTab` (0x00BD11E0: the powers / charms tabs open the
/// objectives when unavailable; `DJT_Sub_ChapterNotes` opens the objectives' chapter notes),
/// as (tab position in `tabs`, sub tab).
pub fn start_tab(list: &[Tab], tab: i32) -> (usize, usize) {
    if tab == 5 {
        return (0, 1);
    }
    let want = match tab {
        1 => Tab::Logs,
        2 => Tab::Powers,
        3 => Tab::BoneCharms,
        4 => Tab::Inventory,
        _ => Tab::Objectives,
    };
    (list.iter().position(|t| *t == want).unwrap_or(0), 0)
}

/// One objectives-list entry (`SetObjectivesList`'s objects: `objState`, `objDescription`,
/// `bIsOptional`, `bHudMarker`, `tasksList`).
#[derive(Clone, Debug, PartialEq)]
pub struct ObjEntry {
    pub state: &'static str,
    pub description: String,
    pub optional: bool,
    pub hud_marker: bool,
    pub tasks: Vec<ObjEntry>,
}

/// `Req_ObjectivesList` (0x00BD79B0) over the mission's objectives:
///
/// - an objective is listed when shown (not hidden; its state 0 / 1 / 2 is "not_done" / "done"
///   / "failed");
/// - its tasks are listed when not hidden and still open (inactive or active: "not_done"), or,
///   with the history shown, finished too ("done" / "failed");
/// - primary objectives come first (0x00BD0A40 inserts them in order), the optional ones after
///   (0x00BC9630 appends);
/// - a task is optional when it or its objective is; its marker is on while it is active.
///
/// The port's objectives carry no state or optional flag of their own: an objective reads
/// "done" once every task it lists is completed (`m_bCompleteWhenAllTasksComplete`, 0x00ACBC70)
/// and optional when all its tasks are.
pub fn objectives_list(objs: &[ObjectiveView], history: bool) -> Vec<ObjEntry> {
    let mut primary = Vec::new();
    let mut optional = Vec::new();
    for o in objs.iter().filter(|o| !o.hidden) {
        let visible: Vec<_> = o.tasks.iter().filter(|t| !t.hidden).collect();
        let done = !visible.is_empty() && visible.iter().all(|t| t.state == TaskState::Completed);
        if done && !history {
            continue;
        }
        let opt = !visible.is_empty() && visible.iter().all(|t| t.optional);
        let tasks = visible
            .iter()
            .filter(|t| history || matches!(t.state, TaskState::Inactive | TaskState::Active))
            .map(|t| ObjEntry {
                state: match t.state {
                    TaskState::Completed => "done",
                    TaskState::Failed => "failed",
                    _ => "not_done",
                },
                description: t.description.clone(),
                optional: t.optional || opt,
                hud_marker: t.state == TaskState::Active,
                tasks: Vec::new(),
            })
            .collect();
        let e = ObjEntry { state: if done { "done" } else { "not_done" }, description: o.description.clone(), optional: opt, hud_marker: !done, tasks };
        if opt { optional.push(e) } else { primary.push(e) }
    }
    primary.extend(optional);
    primary
}

/// A held abstract item as the journal shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemInfo {
    pub name: String,
    pub description: String,
    /// `EDisJournalItemSection`.
    pub section: u8,
    pub icon: String,
}

/// `EDisJournalItemSection` by name (`DisGlobalEnums`).
pub fn journal_section(name: &str) -> Option<u8> {
    const NAMES: [&str; 15] = [
        "DJIS_None",
        "DJIS_Mission",
        "DJIS_Key",
        "DJIS_Resource",
        "DJIS_Gadget",
        "DJIS_Ammo",
        "DJIS_Upgrade",
        "DJIS_AudioLog",
        "DJIS_Note",
        "DJIS_ChapterNote",
        "DJIS_Map",
        "DJIS_BoneCharm",
        "DJIS_Book",
        "DJIS_DLC05_Clue",
        "DJIS_DLC05_NoPickupLog",
    ];
    NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| i as u8)
}

/// The class default `m_JournalDisplaySection` (`DisAbstractItem`: `DJIS_Mission`;
/// `DisAbstractItemAudioLog`: `DJIS_AudioLog`).
pub fn default_section(class: &str) -> u8 {
    if class.eq_ignore_ascii_case("DisAbstractItemAudioLog") { 7 } else { 1 }
}

/// The text of a localized UE3 string (`\n` escapes become line breaks).
pub fn unescape(s: &str) -> String {
    dishonored_assets::loc::plain(&s.replace("\\n", "\n").replace("\\\"", "\""))
}

/// `ConvertVariableTags` on `t_CoinsCount` (`PlayerCoins > 1`) or `t_CoinCount`
/// (`§NB_COINS§`), upper-cased, as `j_ObjectivesScreen` does.
pub fn coins_text(t: &MenuTexts, coins: u32) -> String {
    let key = if coins > 1 { "t_CoinsCount" } else { "t_CoinCount" };
    let raw = t.loc.get(SEC_BASE, key).unwrap_or("§NB_COINS§ Coins");
    raw.replace("§NB_COINS§", &coins.to_string()).to_uppercase()
}

/// Abstract items read from their packages (`<Package>_SF.upk`) and `Localization/INT`.
#[derive(Default)]
pub struct ItemCatalog {
    items: HashMap<String, Option<ItemInfo>>,
    locs: HashMap<String, dishonored_assets::loc::Localization>,
}

impl ItemCatalog {
    /// The item at object path `path` (`Package.Group.Name`), read once.
    pub fn get(&mut self, path: &str) -> Option<&ItemInfo> {
        if !self.items.contains_key(path) {
            let info = self.load(path);
            self.items.insert(path.to_string(), info);
        }
        self.items.get(path).and_then(|i| i.as_ref())
    }

    fn load(&mut self, path: &str) -> Option<ItemInfo> {
        use ue3_package::ObjRef;
        use ue3_package::properties::Value;
        let content = dishonored_assets::Content::locate().ok()?;
        let (pkg_name, rest) = path.split_once('.')?;
        let pkg = [format!("{pkg_name}_SF"), pkg_name.to_string()].into_iter().find(|n| content.has_package(n)).and_then(|n| content.package(&n).ok())?;
        let i = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path))?;
        let class = pkg.export_class(i);
        let h = pkg.read_object(i).ok()?;
        let mut info = ItemInfo { section: default_section(&class), ..Default::default() };
        for p in &h.properties {
            match (p.name.as_str(), &p.value) {
                ("m_ItemName", Value::Str(s)) => info.name = s.clone(),
                ("m_Description", Value::Str(s)) => info.description = s.clone(),
                ("m_JournalIconName", Value::Str(s)) => info.icon = s.clone(),
                ("m_JournalDisplaySection", Value::Enum(s) | Value::Name(s)) => info.section = journal_section(s).unwrap_or(info.section),
                ("m_JournalDisplaySection", Value::Byte(b)) => info.section = *b,
                _ => {}
            }
        }
        // `[<Group.Name> <Class>]` of `<Package>.int` overrides the package's strings.
        let loc = self.locs.entry(pkg_name.to_ascii_lowercase()).or_insert_with(|| dishonored_assets::loc::Localization::load(&content.root, "INT", &[pkg_name]));
        let sec = format!("{rest} {class}");
        if let Some(s) = loc.get(&sec, "m_ItemName") {
            info.name = s.to_string();
        }
        if let Some(s) = loc.get(&sec, "m_Description") {
            info.description = s.to_string();
        }
        info.name = unescape(&info.name);
        info.description = unescape(&info.description);
        Some(info)
    }
}

/// One drawn list row.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Section(String),
    /// Objective (`task` false) or task row.
    Objective { text: String, state: &'static str, task: bool },
    Item { name: String, description: String },
}

impl Row {
    fn selectable(&self) -> bool {
        !matches!(self, Row::Section(_))
    }
}

/// The objectives list's rows (`j_ObjectivesList.InitList` / `CreateListItems`): a section
/// header whenever `bIsOptional` changes, the objective, then its tasks.
pub fn objective_rows(list: &[ObjEntry], primary: &str, optional: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut section: Option<bool> = None;
    for o in list {
        if section != Some(o.optional) {
            section = Some(o.optional);
            rows.push(Row::Section(if o.optional { optional } else { primary }.to_uppercase()));
        }
        rows.push(Row::Objective { text: o.description.clone(), state: o.state, task: false });
        for t in &o.tasks {
            rows.push(Row::Objective { text: t.description.clone(), state: t.state, task: true });
        }
    }
    rows
}

/// Greedy word wrap of `text` at `width` px (`TextField.wordWrap`): the number of lines.
pub fn wrapped_lines(metrics: &FontMetrics, kind: FontKind, text: &str, size: f32, width: f32) -> usize {
    let mut n = 0;
    for para in text.split('\n') {
        let mut line = 0.0f32;
        let mut lines = 1;
        for word in para.split(' ') {
            let w = metrics.width(kind, word, size);
            let sp = metrics.width(kind, " ", size);
            if line > 0.0 && line + sp + w > width {
                lines += 1;
                line = w;
            } else {
                line += if line > 0.0 { sp + w } else { w };
            }
        }
        n += lines;
    }
    n.max(1)
}

/// Seconds `CloseJournal` takes (`_animCloseDuration`).
pub const CLOSE_SECS: f64 = 0.75;
/// `JournalTabContent_tabs._offsetX` and `_tabChangeDuration`.
const CONTENT_SLIDE: f32 = 250.0;
const SUB_CHANGE: f32 = 0.15;
/// `lib_mTabs_tab_s` width and step (`_offsetTabX` -60).
const TAB_W: f32 = 256.0;
const TAB_STEP: f32 = 196.0;
/// `JournalSubTabs._offsetTabMax`.
const SUBTAB_GAP: f32 = 25.0;
/// `j_ItemsListScreen` list and details positions (journal space).
const LIST_POS: Vec2 = Vec2::new(-512.0, -155.0);
const DETAILS_POS: Vec2 = Vec2::new(355.0, -280.0);
/// `j_ObjectivesList._startOriginTaskX`; the text width is the mask's less the field's x and 60.
const TASK_X: f32 = 55.0;
/// Rows the list shows (`_nbLinesView`: objectives 5 entries tall, logs 8).
const LOG_LINES: usize = 8;

#[derive(Resource, Default)]
pub struct Journal {
    pub open: bool,
    root: Option<Entity>,
    cache: LayerCache,
    anim: Anim,
    now: f64,
    opened_at: f64,
    closing: Option<f64>,
    tabs: Vec<Tab>,
    tab: usize,
    sub: usize,
    sel: usize,
    first: usize,
    history: bool,
    /// The content was (re)attached at this time, from this side (`OpenTabContent`).
    content_at: f64,
    content_side: f32,
    /// `DisSeqAct_OpenJournal` activations already handled.
    kismet_seen: u32,
    catalog: ItemCatalog,
    /// Open on the next frame at this `EDisJournalTab` (harness / Kismet).
    pub request: Option<i32>,
    /// Held abstract items of the local player (object paths), the coins, power / charm
    /// availability, set when the journal opens and refreshed while up.
    items: Vec<String>,
    coins: u32,
}

impl Journal {
    fn current(&self) -> Tab {
        self.tabs.get(self.tab).copied().unwrap_or(Tab::Objectives)
    }
}

pub fn setup(mut commands: Commands) {
    let root = menu_art::spawn_layer(&mut commands, 48);
    commands.entity(root).insert((Visibility::Hidden, menu_art::DrawsBlackStripes));
    commands.insert_resource(Journal { root: Some(root), anim: Anim::new(0x10a7), ..default() });
}

/// `JournalScreen` construction + `Open` at `now`.
fn open(j: &mut Journal, art: &MenuArt, sounds: &mut MenuSounds, now: f64, tab: i32) {
    j.open = true;
    j.opened_at = now;
    j.closing = None;
    j.anim.clear();
    let (t, s) = start_tab(&j.tabs, tab);
    j.tab = t;
    j.sub = s;
    j.sel = 0;
    j.first = 0;
    j.history = false;
    let a = &mut j.anim;
    // `_bkgdTabs_mc` from 250 px left and alpha 0 to its place (0.3 s, Strong.easeOut).
    let bt = art.get("journal").and_then(|m| m.export("Journal")).and_then(|ch| art.get("journal").and_then(|m| m.child(ch, "_bkgdTabs_mc")));
    let bp = bt.map(|(_, m)| Props::from_matrix(&m, 44.921875)).unwrap_or_default();
    a.author("j/bkgdTabs", bp);
    a.now(now, "j/bkgdTabs", Act::Set(P::new().x(bp.x - 250.0).alpha(0.0)));
    a.now(now, "j/bkgdTabs", Act::To(P::new().x(bp.x).alpha(bp.alpha), 0.3, Ease::StrongOut));
    // Tabs: one every 50 ms from scale 200 (`TabsHandler`, as the options tabs).
    for i in 0..j.tabs.len() {
        let k = format!("j/tab/{i}");
        a.author(&k, Props::default());
        a.now(now, &k, Act::Set(P::new().visible(false)));
        let at = now + 0.05 * i as f64;
        a.at(at, &k, Act::Set(P::new().scale(200.0).visible(true)));
        a.at(at, &k, Act::To(if i == t { P::new().scale(100.0).y(-20.0) } else { P::new().scale(100.0).y(0.0) }, 0.25, Ease::BackOut));
        a.at(at, &k, Act::Play(if i == t { "selected" } else { "unselected" }.into()));
    }
    a.author("j/content", Props::default());
    a.author("j/screen", Props::default());
    HelpBar::open(a, "j/help", now);
    attach_content(j, now, 1.0);
    sounds.play("OpenScreen");
}

/// `OpenTabContent` / `SetContent`: the content clip slides in from `side` x 250 px (0.25 s).
fn attach_content(j: &mut Journal, now: f64, side: f32) {
    j.content_at = now;
    j.content_side = side;
    j.sel = 0;
    j.first = 0;
    let a = &mut j.anim;
    a.forget("j/content");
    a.author("j/content", Props::default());
    a.now(now, "j/content", Act::Set(P::new().alpha(0.0).x(side * CONTENT_SLIDE)));
    a.now(now, "j/content", Act::To(P::new().alpha(100.0).x(0.0), 0.25, Ease::StrongOut));
    for i in 0..j.tabs.get(j.tab).map(|t| t.subs().len()).unwrap_or(0) {
        let k = format!("j/subtab/{i}");
        a.author(&k, Props::default());
        a.now(now, &k, Act::Play(if i == j.sub { "selected" } else { "unselected" }.into()));
    }
}

/// `CloseJournal`.
fn close(j: &mut Journal, sounds: &mut MenuSounds, now: f64) {
    if j.closing.is_some() {
        return;
    }
    j.closing = Some(now);
    let a = &mut j.anim;
    let bp = a.get("j/bkgdTabs", now);
    a.now(now, "j/bkgdTabs", Act::To(P::new().alpha(0.0).x(bp.x - 150.0), 0.3, Ease::StrongOut));
    a.now(now, "j/content", Act::To(P::new().alpha(0.0).scale(105.0), 0.3, Ease::StrongOut));
    for i in 0..j.tabs.len() {
        a.now(now, &format!("j/tab/{i}"), Act::To(P::new().alpha(0.0), 0.25, Ease::StrongOut));
    }
    HelpBar::close(a, "j/help", now);
    sounds.play("CloseScreen");
}

fn change_tab(j: &mut Journal, to: usize, sounds: &mut MenuSounds, now: f64) {
    if to == j.tab || to >= j.tabs.len() {
        return;
    }
    let side = if to > j.tab { 1.0 } else { -1.0 };
    let a = &mut j.anim;
    a.now(now, &format!("j/tab/{}", j.tab), Act::To(P::new().scale(100.0).y(0.0), 0.25, Ease::BackOut));
    a.now(now, &format!("j/tab/{}", j.tab), Act::Play("unselected".into()));
    a.now(now, &format!("j/tab/{to}"), Act::To(P::new().scale(100.0).y(-20.0), 0.25, Ease::BackOut));
    a.now(now, &format!("j/tab/{to}"), Act::Play("selected".into()));
    j.tab = to;
    j.sub = 0;
    attach_content(j, now, side);
    sounds.play("Move");
}

fn change_sub(j: &mut Journal, to: usize, sounds: &mut MenuSounds, now: f64) {
    let n = j.current().subs().len();
    if to == j.sub || to >= n {
        return;
    }
    let side = if to > j.sub { 1.0 } else { -1.0 };
    j.anim.now(now, &format!("j/subtab/{}", j.sub), Act::Play("unselected".into()));
    j.anim.now(now, &format!("j/subtab/{to}"), Act::Play("selected".into()));
    j.sub = to;
    // `HideContent` (0.15 s), then `SetContent`.
    attach_content(j, now + SUB_CHANGE as f64, side);
    sounds.play("Move");
}

/// The rows of the current sub tab.
fn rows(j: &mut Journal, mv: Option<&dishonored_mission::runtime::MissionView>, t: &MenuTexts) -> Vec<Row> {
    let sub = j.current().subs().get(j.sub).map(|s| s.1);
    match sub {
        Some(Sub::Tasks) => {
            let list = mv.map(|m| objectives_list(&m.objectives, j.history)).unwrap_or_default();
            objective_rows(&list, &jt(t, "t_PrimaryTasks"), &jt(t, "t_SecondaryTasks"))
        }
        Some(Sub::Items(sections)) => {
            let items = j.items.clone();
            items
                .iter()
                .filter_map(|p| j.catalog.get(p).cloned())
                .filter(|i| sections.contains(&i.section))
                .map(|i| Row::Item { name: i.name, description: i.description })
                .collect()
        }
        None => Vec::new(),
    }
}

/// A journal text (its own section, then the shared menu sections).
pub fn jt(t: &MenuTexts, key: &str) -> String {
    match t.loc.get(SEC_JOURNAL, key) {
        Some(s) => dishonored_assets::loc::plain(s),
        None => texts_any(t, key),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn toggle(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    scroll: Res<bevy::input::mouse::AccumulatedMouseScroll>,
    bindings: Option<Res<crate::settings::Bindings>>,
    mut j: ResMut<Journal>,
    mut nav: ResMut<MenuNav>,
    mut local: ResMut<crate::input_map::LocalInput>,
    mut sounds: ResMut<MenuSounds>,
    art: Option<Res<MenuArt>>,
    time: Res<Time<Real>>,
    mut vis: Query<&mut Visibility, With<menu_art::MenuLayer>>,
    (view, mut pause): (Res<crate::session::SessionView>, ResMut<crate::pause_menu::PauseMenu>),
) {
    let now = time.elapsed_secs_f64();
    j.now = now;
    let mv = view.matchv.mission.as_ref();
    // `DisSeqAct_OpenJournal`.
    if let Some((n, tab)) = mv.and_then(|m| m.journal_open) {
        if n > j.kismet_seen {
            j.kismet_seen = n;
            if !j.open {
                j.request = Some(tab);
            }
        }
    }
    let alive = view.me.as_ref().is_some_and(|m| m.alive);
    let pressed = bindings.as_ref().is_some_and(|b| b.just_pressed("GBA_Journal", &keys, &mouse, scroll.delta.y));
    let blocked = pause.open || pause.overlay & !crate::pause_menu::OVERLAY_JOURNAL != 0 || local.override_input.is_some();
    if !j.open && !blocked && mv.is_some() && alive && (pressed || j.request.is_some()) {
        let from_kismet = j.request.take();
        let me = view.me.as_ref();
        j.tabs = tabs(me.is_some_and(|m| m.inv.power_levels.iter().any(|l| *l > 0)), me.is_some_and(|m| m.inv.charms_owned != 0));
        j.coins = me.map(|m| m.inv.coins).unwrap_or(0);
        j.items = view.abstract_items.clone();
        if let Some(a) = art.as_deref() {
            open(&mut j, a, &mut sounds, now, from_kismet.unwrap_or(0));
        } else {
            j.open = true;
        }
        // The player's own opening fires `JournalViewed` on the host (the Kismet's opening
        // already did).
        if from_kismet.is_none() {
            local.action = dishonored_sim::input::actions::OPEN_JOURNAL;
        }
        nav.enabled = false;
        nav.settle = 1;
        nav.focus = None;
        nav.wrap = false;
        nav.horizontal = false;
        local.enabled = false;
    } else if j.open && j.closing.is_none() && pressed {
        // `Dis_ToggleJournal` again.
        close(&mut j, &mut sounds, now);
    }
    if j.open {
        j.anim.advance(now);
        j.items = view.abstract_items.clone();
        nav.enabled = j.closing.is_none();
        nav.wrap = false;
        nav.horizontal = false;
        if let Some(c) = j.closing {
            if now - c >= CLOSE_SECS {
                // `OnClosed` -> `closeJournal`.
                j.open = false;
                j.closing = None;
                nav.enabled = false;
                nav.items.clear();
                local.enabled = true;
            }
        }
    }
    if j.open {
        pause.overlay |= crate::pause_menu::OVERLAY_JOURNAL;
    } else {
        pause.overlay &= !crate::pause_menu::OVERLAY_JOURNAL;
    }
    if let Some(root) = j.root {
        if let Ok(mut v) = vis.get_mut(root) {
            let want = if j.open { Visibility::Inherited } else { Visibility::Hidden };
            if *v != want {
                *v = want;
            }
        }
    }
}

pub fn handle(mut j: ResMut<Journal>, mut nav: ResMut<MenuNav>, mut sounds: ResMut<MenuSounds>, keys: Res<ButtonInput<KeyCode>>, texts: Option<Res<MenuTexts>>, view: Res<crate::session::SessionView>) {
    if !j.open || j.closing.is_some() {
        return;
    }
    let now = j.now;
    let events = std::mem::take(&mut nav.events);
    // `Y` (Alt / Numpad 3, `InputsManager`): the objectives history.
    if keys.any_just_pressed([KeyCode::AltLeft, KeyCode::AltRight, KeyCode::Numpad3]) && j.current().subs().get(j.sub).is_some_and(|s| s.1 == Sub::Tasks) {
        // `YPressed`: `OnToggleObjectivesListHistory`, `J_ToggleHistory`.
        j.history = !j.history;
        j.sel = 0;
        j.first = 0;
        sounds.play("J_ToggleHistory");
    }
    let n_rows = texts.as_deref().map(|t| {
        let mv = view.matchv.mission.clone();
        rows(&mut j, mv.as_ref(), t).iter().filter(|r| r.selectable()).count()
    });
    for ev in events {
        match ev {
            MenuEvent::Back => {
                // `BPressed`: `back`, `CloseJournal`.
                sounds.play("back");
                close(&mut j, &mut sounds, now);
                return;
            }
            MenuEvent::Button("LB") => {
                let to = j.tab.checked_sub(1);
                if let Some(to) = to {
                    change_tab(&mut j, to, &mut sounds, now);
                }
            }
            MenuEvent::Button("RB") => {
                let to = j.tab + 1;
                change_tab(&mut j, to, &mut sounds, now);
            }
            MenuEvent::Button("LT") => {
                if let Some(to) = j.sub.checked_sub(1) {
                    change_sub(&mut j, to, &mut sounds, now);
                }
            }
            MenuEvent::Button("RT") => {
                let to = j.sub + 1;
                change_sub(&mut j, to, &mut sounds, now);
            }
            MenuEvent::Step { dir, .. } => {
                let to = j.tab as i32 + dir;
                if to >= 0 {
                    change_tab(&mut j, to as usize, &mut sounds, now);
                }
            }
            MenuEvent::Activate { id, .. } => {
                if let Some(i) = id.strip_prefix("j:tab:").and_then(|s| s.parse().ok()) {
                    change_tab(&mut j, i, &mut sounds, now);
                } else if let Some(i) = id.strip_prefix("j:sub:").and_then(|s| s.parse().ok()) {
                    change_sub(&mut j, i, &mut sounds, now);
                } else if let Some(i) = id.strip_prefix("j:row:").and_then(|s| s.parse::<usize>().ok()) {
                    j.sel = i;
                } else if id == "help:B" {
                    sounds.play("back");
                    close(&mut j, &mut sounds, now);
                    return;
                }
            }
            MenuEvent::Focus(id) => {
                if let Some(i) = id.strip_prefix("j:row:").and_then(|s| s.parse::<usize>().ok()) {
                    if i != j.sel {
                        j.sel = i;
                        sounds.play("Move");
                    }
                }
            }
            MenuEvent::Scroll(d) => {
                let n = n_rows.unwrap_or(0);
                let to = (j.sel as i32 + d).clamp(0, n.saturating_sub(1) as i32) as usize;
                if to != j.sel {
                    j.sel = to;
                    sounds.play("Move");
                }
            }
            MenuEvent::Button("Stop") => sounds.play("Stop"),
            _ => {}
        }
    }
    if let Some(n) = n_rows {
        j.sel = j.sel.min(n.saturating_sub(1));
    }
}

/// The content clip's stage matrix (journal origin, the content tween).
fn content_matrix(j: &Journal, now: f64) -> (Matrix, f32) {
    let p = j.anim.get("j/content", now);
    let m = px_matrix(640.0, 360.0).mul(&Props { x: p.x, ..Props::default() }.matrix()).mul(&Matrix { a: p.xscale / 100.0, d: p.yscale / 100.0, ..Matrix::default() });
    (m, p.alpha / 100.0)
}

fn build(j: &mut Journal, art: &MenuArt, t: &MenuTexts, mv: Option<&dishonored_mission::runtime::MissionView>, now: f64) -> DisplayList {
    let mut dl = DisplayList::default();
    let Some(jm) = art.get("journal") else { return dl };
    let origin = px_matrix(640.0, 360.0);
    let clock = (now - j.opened_at) as f32;
    // `bkgd_mc` (`p_bkgd`: white noise, particles, blades).
    if let Some((ch, m)) = jm.root_child("bkgd_mc") {
        dl.movie_fx(jm, ch, m, &States::default().clock(clock), 1.0, &|_| None, false);
    }
    // The tab's content clip (left panel art) and the `Journal` frame with `_bkgdTabs_mc`.
    let (cm, ca) = content_matrix(j, now);
    let tab = j.current();
    if let Some(ch) = jm.export(tab.linkage()) {
        dl.movie(jm, ch, cm, &States::default(), ca);
    }
    if let Some(ch) = jm.export("Journal") {
        let bt = j.anim.get("j/bkgdTabs", now);
        let ovr = move |p: &str| if p == "_bkgdTabs_mc" { Some(bt) } else { None };
        dl.movie_fx(jm, ch, origin, &States::default(), 1.0, &ovr, false);
    }
    draw_tabs(&mut dl, j, art, t, now);
    draw_subtabs(&mut dl, j, jm, art, t, now, cm, ca);
    let rows = rows(j, mv, t);
    match tab.subs().get(j.sub).map(|s| s.1) {
        Some(Sub::Tasks) => {
            draw_objectives(&mut dl, j, jm, &art.metrics, t, &rows, now, cm, ca);
            draw_chapter(&mut dl, jm, mv, cm, ca);
        }
        Some(Sub::Items(_)) => draw_items(&mut dl, j, jm, t, &rows, now, cm, ca),
        None => {
            dl.text(texts_any(t, "t_EmptyList"), Rect::new(0.0, 330.0, 1280.0, 370.0), 26.0, FontKind::Normal, TEXT.with_alpha(ca), Justify::Center);
        }
    }
    // Coins (`j_obj_moneyCart_` at (-590, 288) of the objectives screen).
    if tab == Tab::Objectives {
        if let Some(ch) = jm.export("j_obj_moneyCart_") {
            let m = cm.mul(&px_matrix(-590.0, 288.0));
            dl.movie(jm, ch, m, &States::default(), ca);
            if let Some(f) = DisplayList::fields(jm, ch, m, &States::default()).first() {
                dl.field_text(jm, f, &coins_text(t, j.coins), None);
            }
        }
    }
    // Vignette.
    if let Some(v) = jm.export("m_Vignette") {
        dl.movie(jm, v, origin, &States::default(), 1.0);
    }
    if let Some(c) = art.get("common") {
        let help: Vec<(&str, &str)> = match tab.subs().get(j.sub).map(|s| s.1) {
            Some(Sub::Tasks) => vec![("Y", if j.history { "t_HideHistory" } else { "t_ShowHistory" }), ("B", "t_Exit")],
            _ => vec![("B", "t_Exit")],
        };
        HelpBar::draw(&mut dl, c, &art.metrics, t, &j.anim, "j/help", now, &help);
    }
    dl
}

fn draw_tabs(dl: &mut DisplayList, j: &Journal, art: &MenuArt, t: &MenuTexts, now: f64) {
    let Some(common) = art.get("common") else { return };
    let Some(tab) = common.export("lib_mTabs_tab_s") else { return };
    let n = j.tabs.len();
    let w_all = TAB_W * n as f32 - (TAB_W - TAB_STEP) * (n as f32 - 1.0);
    let tabs_m = px_matrix(640.0, 100.0);
    for (i, tb) in j.tabs.iter().enumerate() {
        let k = format!("j/tab/{i}");
        let p = j.anim.get(&k, now);
        if !p.visible || p.alpha <= 0.0 {
            continue;
        }
        let cx = -w_all * 0.5 + TAB_W * 0.5 + TAB_STEP * i as f32;
        let m = tabs_m.mul(&px_matrix(cx, 0.0)).mul(&Props { x: 0.0, ..p }.matrix());
        let (label, secs) = j.anim.timeline(&k, now).map(|(l, s)| (l.to_string(), s)).unwrap_or(("unselected".into(), 0.0));
        let st = States::default().with_timed("tab_mc", &label, secs).with_timed("tab_mc/glow_mc", "loop", (now - j.opened_at) as f32);
        let a = p.alpha / 100.0;
        dl.movie(common, tab, m, &st, a);
        let text = texts_any(t, tb.label_key()).to_uppercase();
        for f in DisplayList::fields(common, tab, m, &st) {
            let mut f = f;
            f.color.mul[3] *= a;
            dl.field_text(common, &f, &text, None);
        }
        dl.item(format!("j:tab:{i}"), Rect::from_center_size(Vec2::new(m.tx, m.ty) / 20.0, Vec2::new(180.0, 50.0)), false);
    }
    // Shoulders (LB / RB) beside the tabs.
    for (name, x) in [("lib_LShoulder_Multi", -(w_all * 0.5 + 15.0)), ("lib_RShoulder_Multi", w_all * 0.5 + 15.0)] {
        if let Some(ic) = common.export(name) {
            dl.movie(common, ic, tabs_m.mul(&px_matrix(x, 0.0)), &States::all("PC"), 1.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_subtabs(dl: &mut DisplayList, j: &Journal, jm: &MovieArt, art: &MenuArt, t: &MenuTexts, now: f64, cm: Matrix, ca: f32) {
    let subs = j.current().subs();
    if subs.is_empty() {
        return;
    }
    let base = cm.mul(&px_matrix(-250.0, -185.0));
    if let Some(bk) = jm.export("j_SubTabsBkgd") {
        dl.movie(jm, bk, base, &States::default(), ca);
    }
    let Some(st_ch) = jm.export("SubTab") else { return };
    let labels: Vec<String> = subs.iter().map(|(k, _)| jt(t, k).to_uppercase()).collect();
    let widths: Vec<f32> = labels.iter().map(|l| art.metrics.field_width(FontKind::Normal, l, 25.0)).collect();
    // Left edge at x -544 (`JournalSubTabs.SetTabs`).
    let mut x = -544.0;
    for (i, label) in labels.iter().enumerate() {
        let cx = x + widths[i] * 0.5;
        x += widths[i] + SUBTAB_GAP;
        let k = format!("j/subtab/{i}");
        let (lab, secs) = j.anim.timeline(&k, now).map(|(l, s)| (l.to_string(), s)).unwrap_or(("unselected".into(), 0.0));
        let m = cm.mul(&px_matrix(cx, -185.0));
        let st = States::default().with_timed("tab_mc", &lab, secs);
        dl.movie(jm, st_ch, m, &st, ca);
        for f in DisplayList::fields(jm, st_ch, m, &st) {
            let mut f = f;
            f.color.mul[3] *= ca;
            dl.field_text(jm, &f, label, None);
        }
        dl.item(format!("j:sub:{i}"), Rect::from_center_size(Vec2::new(m.tx, m.ty) / 20.0, Vec2::new(widths[i], 34.0)), false);
    }
}

/// Width of a list's `maskContent` (the text width the rows wrap to).
fn mask_width(jm: &MovieArt, list: &str) -> f32 {
    jm.export(list).and_then(|ch| jm.child(ch, "maskContent")).and_then(|(c, m)| jm.bounds(c, m, &States::default())).map(|r| r.width()).unwrap_or(500.0)
}

#[allow(clippy::too_many_arguments)]
fn draw_objectives(dl: &mut DisplayList, j: &mut Journal, jm: &MovieArt, metrics: &FontMetrics, t: &MenuTexts, rows: &[Row], now: f64, cm: Matrix, ca: f32) {
    let lm = cm.mul(&px_matrix(LIST_POS.x, LIST_POS.y));
    if let Some(ch) = jm.export("j_obj_objectivesList_list") {
        dl.movie(jm, ch, lm, &States::default(), ca);
    }
    if rows.is_empty() {
        let m = lm.mul(&px_matrix(0.0, 30.0));
        dl.text(jt(t, "t_EmptyObjectivesList"), Rect::new(m.tx / 20.0, m.ty / 20.0 - 20.0, m.tx / 20.0 + 600.0, m.ty / 20.0 + 20.0), 24.0, FontKind::Normal, TEXT.with_alpha(ca), Justify::Left);
        return;
    }
    let mask_w = mask_width(jm, "j_obj_objectivesList_list");
    let (Some(sec_ch), Some(item_ch)) = (jm.export("j_obj_objectivesList_section"), jm.export(" j_obj_objectivesList_taskItem")) else { return };
    let sec_h = jm.bounds(sec_ch, Matrix::default(), &States::default()).map(|r| r.height()).unwrap_or(40.0);
    // The task text field (`mc/mc/txt`, `$NormalFont` 24 px): one line is the field's height.
    let line_h = 27.25;
    let mut y_prev: Option<(f32, f32)> = None; // (y, height) of the previous clip
    let mut after_section = false;
    let mut sel_idx = 0usize;
    let view_h = 5.0 * 64.0;
    // Positions first (`CreateListItems`), then scroll so the selection is in view.
    let mut placed: Vec<(usize, f32, f32)> = Vec::new();
    for (ri, r) in rows.iter().enumerate() {
        match r {
            Row::Section(_) => {
                let y = match y_prev {
                    None => 0.0,
                    Some((py, ph)) => py + 0.5 * ph + 20.0,
                };
                placed.push((ri, y, sec_h));
                y_prev = Some((y, sec_h));
                after_section = true;
            }
            Row::Objective { text, task, .. } => {
                let x0 = if *task { TASK_X } else { 0.0 };
                let w = mask_w - x0 - 60.0;
                let lines = wrapped_lines(metrics, FontKind::Normal, text, 24.0, w.max(100.0));
                let h = lines as f32 * line_h + 10.0;
                let y = match (y_prev, after_section) {
                    (None, _) => 0.5 * h,
                    (Some((py, ph)), true) => py + ph + 10.0 + 0.5 * h,
                    (Some((py, ph)), false) => py + 0.5 * ph + if *task { 0.0 } else { 20.0 } + 0.5 * h,
                };
                placed.push((ri, y, h));
                y_prev = Some((y, h));
                after_section = false;
            }
            Row::Item { .. } => {}
        }
    }
    let sel_y = placed.iter().filter(|(ri, _, _)| rows[*ri].selectable()).nth(j.sel).map(|p| p.1).unwrap_or(0.0);
    let scroll = (sel_y - view_h + 40.0).max(0.0);
    for (ri, y, h) in placed {
        let y = y - scroll;
        if !(-10.0..=view_h + 20.0).contains(&y) {
            if rows[ri].selectable() {
                sel_idx += 1;
            }
            continue;
        }
        match &rows[ri] {
            Row::Section(s) => {
                let m = lm.mul(&px_matrix(0.5 * mask_w, y));
                dl.movie(jm, sec_ch, m, &States::default(), ca);
                for f in DisplayList::fields(jm, sec_ch, m, &States::default()) {
                    dl.field_text(jm, &f, s, None);
                }
            }
            Row::Objective { text, state, task } => {
                let i = sel_idx;
                sel_idx += 1;
                let selected = i == j.sel;
                let x0 = if *task { TASK_X } else { 0.0 } + if selected { 10.0 } else { 0.0 };
                let m = lm.mul(&px_matrix(x0, y));
                let lock = matches!(*state, "done" | "failed");
                let label = match (selected, lock) {
                    (true, false) => "over",
                    (true, true) => "over_lock",
                    (false, true) => "stop_lock",
                    _ => "default",
                };
                let st = States::default().with("*", label).with("mc/mc/ic", state);
                // `mc.mc._y = -0.5 * mc._height`: the text block is centred on the row.
                let text_h = h - 10.0;
                let ovr = move |p: &str| if p == "mc/mc" { Some(Props { y: -0.5 * text_h, ..Props::default() }) } else { None };
                dl.movie_fx(jm, item_ch, m, &st, ca, &ovr, false);
                let fields = DisplayList::fields_fx(jm, item_ch, m, &st, &ovr);
                if let Some(f) = fields.iter().find(|f| f.path.ends_with("txt")) {
                    let (x0s, y0s, _, _) = f.stage_rect();
                    let w = mask_w - (if *task { TASK_X } else { 0.0 }) - 60.0;
                    let col = if lock { menu_art::TEXT_DIM } else { TEXT };
                    dl.paragraph(text.clone(), Rect::new(x0s, y0s, x0s + w, y0s + text_h + 4.0), 24.0, FontKind::Normal, col.with_alpha(ca), Justify::Left);
                }
                let (cx, cy) = (m.tx / 20.0, m.ty / 20.0);
                dl.item(format!("j:row:{i}"), Rect::new(cx, cy - 0.5 * h, cx + mask_w - x0, cy + 0.5 * h), true);
            }
            Row::Item { .. } => {}
        }
    }
    let _ = now;
}

/// `j_obj_chapterDetails`: the chapter's title and description (`SetChapterInfos`).
fn draw_chapter(dl: &mut DisplayList, jm: &MovieArt, mv: Option<&dishonored_mission::runtime::MissionView>, cm: Matrix, ca: f32) {
    let Some(ch) = jm.export("j_obj_chapterDetails") else { return };
    let m = cm.mul(&px_matrix(DETAILS_POS.x, DETAILS_POS.y));
    dl.movie(jm, ch, m, &States::default(), ca);
    let (title, desc) = mv.map(|v| (v.title.clone(), v.briefing.clone())).unwrap_or_default();
    details_text(dl, jm, ch, m, ca, &title, &desc);
}

/// Title into the details clip's first `$TitleFont` field, the description wrapped below it in
/// the scroll view's width.
fn details_text(dl: &mut DisplayList, jm: &MovieArt, ch: u16, m: Matrix, ca: f32, title: &str, desc: &str) {
    let fields = DisplayList::fields(jm, ch, m, &States::default());
    let is_title = |f: &dishonored_assets::swf::TextPlacement| f.text.font.and_then(|id| jm.movie.font_name(id)).is_some_and(|n| n.contains("Title"));
    let title_f = fields.iter().find(|f| is_title(f));
    if let Some(f) = title_f {
        let mut f = f.clone();
        f.color.mul[3] *= ca;
        dl.field_text(jm, &f, &title.to_uppercase(), None);
    }
    // The scroll view (`_scrollView_mc` at its authored place): its mask bounds the text.
    let rect = jm
        .child(ch, "_scrollView_mc")
        .and_then(|(sc, sm)| jm.child(sc, "_scrollView_mask_mc").map(|(mc, mm)| (mc, m.mul(&sm).mul(&mm))))
        .and_then(|(mc, mm)| jm.bounds(mc, mm, &States::default()));
    let body = desc.to_string();
    let r = match rect {
        Some(r) => r,
        None => {
            let (x, y) = (m.tx / 20.0, m.ty / 20.0);
            Rect::new(x - 190.0, y + 80.0, x + 190.0, y + 520.0)
        }
    };
    let top = title_f.map(|f| f.stage_rect().3 + 10.0).unwrap_or(r.min.y).max(r.min.y);
    dl.paragraph(body, Rect::new(r.min.x + 4.0, top, r.max.x - 4.0, r.max.y.max(top + 40.0)), 22.0, FontKind::Normal, TEXT.with_alpha(ca), Justify::Left);
}

#[allow(clippy::too_many_arguments)]
fn draw_items(dl: &mut DisplayList, j: &mut Journal, jm: &MovieArt, t: &MenuTexts, rows: &[Row], now: f64, cm: Matrix, ca: f32) {
    let lm = cm.mul(&px_matrix(LIST_POS.x, LIST_POS.y));
    if let Some(ch) = jm.export("j_logs_logsList") {
        dl.movie(jm, ch, lm, &States::default(), ca);
    }
    let Some(item_ch) = jm.export("j_logs_logsList_item") else { return };
    if rows.is_empty() {
        // `SetEmptyList` (`j_itemsList_empty`, `t_EmptyList`).
        if let Some(e) = jm.export("j_itemsList_empty") {
            let m = lm.mul(&px_matrix(0.5 * mask_width(jm, "j_logs_logsList"), 40.0));
            dl.movie(jm, e, m, &States::default(), ca);
            if let Some(f) = DisplayList::fields(jm, e, m, &States::default()).first() {
                dl.field_text(jm, f, &texts_any(t, "t_EmptyList"), None);
            }
        }
        return;
    }
    let row_h = jm.bounds(item_ch, Matrix::default(), &States::all("default")).map(|r| r.height()).unwrap_or(56.0);
    let n = rows.len();
    j.first = j.first.min(n.saturating_sub(1));
    if j.sel < j.first {
        j.first = j.sel;
    } else if j.sel >= j.first + LOG_LINES {
        j.first = j.sel + 1 - LOG_LINES;
    }
    for (vi, i) in (j.first..(j.first + LOG_LINES).min(n)).enumerate() {
        let Row::Item { name, .. } = &rows[i] else { continue };
        let selected = i == j.sel;
        let y = 0.5 * row_h + vi as f32 * row_h;
        let m = lm.mul(&px_matrix(if selected { 20.0 } else { 0.0 }, y));
        // `_bkgd_mc/mc` cycles its frames row by row (`SetList`).
        let st = States::all(if selected { "over" } else { "default" }).with("_bkgd_mc/mc", &format!("#{}", i % 4));
        dl.movie(jm, item_ch, m, &st, ca);
        for f in DisplayList::fields(jm, item_ch, m, &st) {
            if f.path.contains("txt") {
                let mut f = f;
                f.color.mul[3] *= ca;
                dl.field_text(jm, &f, name, None);
            }
        }
        let (cx, cy) = (m.tx / 20.0, m.ty / 20.0);
        dl.item(format!("j:row:{i}"), Rect::new(cx, cy - 0.5 * row_h, cx + 500.0, cy + 0.5 * row_h), true);
    }
    // Details of the selection (`Req_LogInfo` -> `SetDetails`).
    if let Some(Row::Item { name, description }) = rows.get(j.sel) {
        if let Some(ch) = jm.export("j_logs_logDetails") {
            let m = cm.mul(&px_matrix(DETAILS_POS.x, DETAILS_POS.y));
            dl.movie(jm, ch, m, &States::default(), ca);
            details_text(dl, jm, ch, m, ca, name, description);
        }
    }
    let _ = now;
}

#[allow(clippy::too_many_arguments)]
pub fn rebuild(mut commands: Commands, mut j: ResMut<Journal>, art: Option<Res<MenuArt>>, t: Option<Res<MenuTexts>>, mut nav: ResMut<MenuNav>, fonts: Option<Res<crate::gfx_hud::GameFonts>>, view: Res<crate::session::SessionView>) {
    let (Some(art), Some(t), Some(root)) = (art, t, j.root) else { return };
    if !j.open {
        return;
    }
    let now = j.now;
    let mv = view.matchv.mission.clone();
    let dl = build(&mut j, &art, &t, mv.as_ref(), now);
    nav.set_items(dl.items.clone());
    let want = format!("j:row:{}", j.sel);
    if nav.items.iter().any(|i| i.id == want) && nav.focus.as_deref() != Some(want.as_str()) {
        nav.focus = Some(want);
    }
    let j = &mut *j;
    menu_art::spawn_list_cached(&mut commands, root, &dl, fonts.as_deref(), &mut j.cache);
}
