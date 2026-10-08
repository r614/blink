//! The original loading screen (`DisBinkOverlayManager`): a Bink movie per map with the map name,
//! rotating hints and "LOADING" drawn over it in the loading package's bitmap fonts.
//!
//! Data, all read from the install at runtime:
//! - `DefaultEngine.ini [DishonoredGame.DishonoredEngine] m_MapConfig`: per map the movie
//!   (`m_LoadingMovieName`, `Movies/<name>.bik`) and `m_bShowLoadingTexts`;
//! - `DefaultUI.ini [DishonoredGame.DisBinkOverlayManager]`: the text layouts
//!   (`DisBinkTextOverlayInfo`: position, wrap size, rotation in degrees, scale, colour), the
//!   map name shadow colour, `m_fHintDuration` and `m_MapHintBindings` (map -> hint set);
//! - `Localization/INT/DishonoredGame.int`: `[MapNames]`, `[DisBinkOverlayManager]` (font
//!   names, `m_LoadingText`) and `[DisBinkOverlayManager_HintSet_<Set>] Hint=` lines;
//! - `UI_Loading_SF_LOC_INT`: the `Emerge_BF` (title) and `ChaletComprime-CologneEighty`
//!   (normal) fonts.
//! The Bink movie is decoded by an `ffmpeg` process (PATH or `DISHONORED_FFMPEG`) streaming
//! RGBA frames at the movie's rate; without ffmpeg the overlay is drawn on black.
//! An offline match does not tick while the screen is up (the original loads synchronously).

use crate::map_render;
use crate::menu_art::{self, StagePrim};
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use dishonored_assets::font::BitmapFont;
use dishonored_assets::ini::Ini;
use std::io::Read;
use std::sync::{Arc, Mutex};

const W: u32 = 1280;
const H: u32 = 720;

#[derive(Clone, Default)]
struct TextInfo {
    wrapped: bool,
    right: bool,
    pos: Vec2,
    size: Vec2,
    rot_deg: f32,
    scale: Vec2,
    color: Color,
}

struct Config {
    root: std::path::PathBuf,
    engine: Ini,
    ui: Ini,
    loc: Ini,
    title: Option<FontGpu>,
    normal: Option<FontGpu>,
}

struct FontGpu {
    font: BitmapFont,
    pages: Vec<Handle<Image>>,
}

#[derive(Resource, Default)]
pub struct LoadingScreen {
    /// The screen is up (an offline match stops ticking).
    pub active: bool,
    cfg: Option<Arc<Config>>,
    root: Option<Entity>,
    video: Option<Video>,
    hints: Vec<String>,
    hint: usize,
    hint_t: f32,
    hint_duration: f32,
    texts_root: Option<Entity>,
    show_texts: bool,
    map: String,
    shown_for: f32,
    want_end: bool,
}

struct Video {
    frame: Arc<Mutex<Option<Vec<u8>>>>,
    child: std::process::Child,
    image: Handle<Image>,
}

impl Drop for Video {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// `(a=1,b=(X=2,Y=3))` field lookup: the text of `key`'s value (nested parentheses kept).
fn field<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let mut depth = 0;
    let b = s.as_bytes();
    let k = key.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ if depth == 1 && b[i..].starts_with(k) && b.get(i + k.len()) == Some(&b'=') && (i == 0 || matches!(b[i - 1], b'(' | b',')) => {
                let start = i + k.len() + 1;
                let mut d = 0;
                let mut j = start;
                while j < b.len() {
                    match b[j] {
                        b'(' => d += 1,
                        b')' if d == 0 => break,
                        b')' => d -= 1,
                        b',' if d == 0 => break,
                        _ => {}
                    }
                    j += 1;
                }
                return Some(s[start..j].trim().trim_matches('"'));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn num(s: Option<&str>) -> f32 {
    s.and_then(|v| v.trim().parse().ok()).unwrap_or(0.0)
}

fn vec2(s: Option<&str>) -> Vec2 {
    let s = s.unwrap_or("");
    let w = format!("({})", s.trim_start_matches('(').trim_end_matches(')'));
    Vec2::new(num(field(&w, "X")), num(field(&w, "Y")))
}

fn text_info(ui: &Ini, key: &str) -> TextInfo {
    let raw = ui.get("DishonoredGame.DisBinkOverlayManager", key).unwrap_or("()");
    let c = field(raw, "m_Color").unwrap_or("");
    let cw = format!("({})", c.trim_start_matches('(').trim_end_matches(')'));
    let ch = |k: &str| num(field(&cw, k)) / 255.0;
    let flag = |k: &str| field(raw, k).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    TextInfo {
        wrapped: flag("m_bWrapped"),
        right: flag("m_bRightAligned"),
        pos: vec2(field(raw, "m_Position")),
        size: vec2(field(raw, "m_Size")),
        rot_deg: num(field(raw, "m_fRotation")),
        scale: vec2(field(raw, "m_Scale")),
        color: Color::srgba(ch("R"), ch("G"), ch("B"), if field(&cw, "A").is_some() { ch("A") } else { 1.0 }),
    }
}

fn load_config(images: &mut Assets<Image>) -> Option<Config> {
    let content = dishonored_assets::Content::locate().ok()?;
    let root = content.cooked.parent()?.parent()?.to_path_buf();
    let engine = Ini::load_layered(&root, "Engine");
    let ui = Ini::load_layered(&root, "UI");
    let mut loc = Ini::default();
    if let Ok(b) = std::fs::read(root.join("DishonoredGame/Localization/INT/DishonoredGame.int")) {
        loc.parse_into(&dishonored_assets::loc::decode(&b));
    }
    let font = |name: &str, images: &mut Assets<Image>| -> Option<FontGpu> {
        let f = dishonored_assets::font::load_font(&content, "UI_Loading_SF_LOC_INT", &format!("UI_Loading.{name}")).map_err(|e| warn!("loading font {name}: {e:#}")).ok()?;
        let pages = f.pages.iter().map(|t| images.add(map_render::to_image(t))).collect();
        Some(FontGpu { font: f, pages })
    };
    let title = loc.get("DisBinkOverlayManager", "m_TitleFontName").map(|s| s.to_string()).and_then(|n| font(&n, images));
    let normal = loc.get("DisBinkOverlayManager", "m_NormalFontName").map(|s| s.to_string()).and_then(|n| font(&n, images));
    Some(Config { root, engine, ui, loc, title, normal })
}

/// `m_MapConfig` entry for a map: (movie name, show loading texts).
fn map_config(cfg: &Config, map: &str) -> (Option<String>, bool) {
    for v in cfg.engine.sections.get("DishonoredGame.DishonoredEngine").and_then(|s| s.get("m_MapConfig")).into_iter().flatten() {
        if field(v, "m_Name").is_some_and(|n| n.eq_ignore_ascii_case(map)) {
            let movie = field(v, "m_LoadingMovieName").filter(|m| !m.is_empty()).map(|m| m.to_string());
            return (movie, field(v, "m_bShowLoadingTexts").is_some_and(|b| b.eq_ignore_ascii_case("true")));
        }
    }
    (None, false)
}

fn hints_for(cfg: &Config, map: &str) -> Vec<String> {
    let set = cfg
        .ui
        .sections
        .get("DishonoredGame.DisBinkOverlayManager")
        .and_then(|s| s.get("m_MapHintBindings"))
        .into_iter()
        .flatten()
        .find(|v| field(v, "m_MapName").is_some_and(|n| n.eq_ignore_ascii_case(map)))
        .and_then(|v| field(v, "m_HintSet").map(|s| s.to_string()));
    let Some(set) = set else { return Vec::new() };
    let section = format!("DisBinkOverlayManager_HintSet_{set}");
    let get = |k: &str| cfg.loc.sections.get(&section).and_then(|s| s.get(k)).cloned().unwrap_or_default();
    // Generic hints plus the keyboard variants (this is the PC build).
    get("Hint").into_iter().chain(get("Hint_Keyboard")).map(|h| h.trim().trim_matches('"').to_string()).filter(|h| !h.is_empty()).collect()
}

fn start_video(cfg: &Config, movie: &str, images: &mut Assets<Image>) -> Option<Video> {
    let path = cfg.root.join("DishonoredGame/Movies").join(format!("{movie}.bik"));
    if !path.exists() {
        return None;
    }
    let ffmpeg = std::env::var("DISHONORED_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
    let mut child = std::process::Command::new(ffmpeg)
        .args(["-loglevel", "error", "-re", "-stream_loop", "-1", "-i"])
        .arg(&path)
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-vf", &format!("scale={W}:{H}"), "pipe:1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| warn!("loading movie {movie}: ffmpeg not available ({e}); overlay on black"))
        .ok()?;
    info!("loading movie {}", path.display());
    let mut out = child.stdout.take()?;
    let frame = Arc::new(Mutex::new(None));
    let slot = frame.clone();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; (W * H * 4) as usize];
        while out.read_exact(&mut buf).is_ok() {
            *slot.lock().unwrap() = Some(buf.clone());
        }
    });
    let img = Image::new_fill(Extent3d { width: W, height: H, depth_or_array_layers: 1 }, TextureDimension::D2, &[0, 0, 0, 255], TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::default());
    Some(Video { frame, child, image: images.add(img) })
}

/// Lay out one text the way the overlay manager draws it: glyph quads in stage space, scaled,
/// right-aligned or wrapped, then rotated about the text position.
fn text_quads(commands: &mut Commands, parent: Entity, f: &FontGpu, text: &str, info: &TextInfo, color: Color, offset: Vec2) {
    let (sx, sy) = (if info.scale.x > 0.0 { info.scale.x } else { 1.0 }, if info.scale.y > 0.0 { info.scale.y } else { 1.0 });
    let line_h = f.font.line_height as f32 * sy;
    // Word wrap to the configured width (stage units).
    let mut lines: Vec<String> = Vec::new();
    if info.wrapped && info.size.x > 0.0 {
        let mut cur = String::new();
        for w in text.split_whitespace() {
            let cand = if cur.is_empty() { w.to_string() } else { format!("{cur} {w}") };
            if !cur.is_empty() && f.font.width(&cand) as f32 * sx > info.size.x {
                lines.push(std::mem::take(&mut cur));
                cur = w.to_string();
            } else {
                cur = cand;
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    } else {
        lines.push(text.to_string());
    }
    let (s, c) = info.rot_deg.to_radians().sin_cos();
    let origin = info.pos;
    let rot = |p: Vec2| {
        let d = p - origin;
        origin + Vec2::new(c * d.x - s * d.y, s * d.x + c * d.y) + offset
    };
    for (li, line) in lines.iter().enumerate() {
        let width = f.font.width(line) as f32 * sx;
        let mut x = if info.right { origin.x - width } else { origin.x };
        let y0 = origin.y + li as f32 * line_h;
        for ch in line.chars() {
            let Some(g) = f.font.glyph(ch) else { continue };
            let (w, h) = (g.w as f32 * sx, g.h as f32 * sy);
            if g.w > 0 && g.h > 0 {
                let y = y0 + g.y_offset as f32 * sy;
                let corners = [rot(Vec2::new(x, y)), rot(Vec2::new(x + w, y)), rot(Vec2::new(x + w, y + h)), rot(Vec2::new(x, y + h))];
                let Some(page) = f.pages.get(g.page as usize) else { continue };
                commands.spawn((
                    ImageNode { image: page.clone(), rect: Some(Rect::new(g.u as f32, g.v as f32, (g.u + g.w) as f32, (g.v + g.h) as f32)), color, ..default() },
                    Node { position_type: PositionType::Absolute, ..default() },
                    UiTransform::default(),
                    StagePrim::Quad(corners.map(|p| (p.x, p.y))),
                    Pickable::IGNORE,
                    ChildOf(parent),
                ));
            }
            x += (g.w + f.font.kerning) as f32 * sx;
        }
    }
}

impl LoadingScreen {
    /// Bring the screen up for a level about to load.
    pub fn begin(&mut self, commands: &mut Commands, images: &mut Assets<Image>, map: &str) {
        self.end_now(commands);
        if self.cfg.is_none() {
            self.cfg = load_config(images).map(Arc::new);
        }
        let backdrop = commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, BackgroundColor(Color::BLACK), GlobalZIndex(95), Pickable::IGNORE)).id();
        let layer = menu_art::spawn_layer(commands, 96);
        commands.entity(layer).insert(ChildOf(backdrop));
        self.root = Some(backdrop);
        self.active = true;
        self.map = map.to_string();
        self.shown_for = 0.0;
        self.want_end = false;
        self.hint = 0;
        self.hint_t = 0.0;
        let Some(cfg) = self.cfg.clone() else { return };
        let (movie, show_texts) = map_config(&cfg, map);
        self.show_texts = show_texts;
        self.video = movie.and_then(|m| start_video(&cfg, &m, images));
        if let Some(v) = &self.video {
            commands.spawn((
                ImageNode::new(v.image.clone()),
                Node { position_type: PositionType::Absolute, ..default() },
                UiTransform::default(),
                StagePrim::Quad([(0.0, 0.0), (W as f32, 0.0), (W as f32, H as f32), (0.0, H as f32)]),
                Pickable::IGNORE,
                ChildOf(layer),
            ));
        }
        self.hints = hints_for(&cfg, map);
        // Start on a hint picked from the clock, as the original picks one at random.
        if !self.hints.is_empty() {
            self.hint = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) as usize) % self.hints.len();
        }
        self.hint_duration = cfg.ui.get_f32("DishonoredGame.DisBinkOverlayManager", "m_fHintDuration").unwrap_or(7.0);
        let texts = commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, ChildOf(layer), Pickable::IGNORE)).id();
        self.texts_root = Some(texts);
        self.draw_texts(commands);
    }

    fn draw_texts(&self, commands: &mut Commands) {
        let (Some(cfg), Some(root)) = (self.cfg.as_ref(), self.texts_root) else { return };
        commands.entity(root).despawn_related::<Children>();
        let normal = cfg.normal.as_ref();
        if let Some(f) = normal {
            let info = text_info(&cfg.ui, "m_LoadingTextInfo");
            let t = cfg.loc.get("DisBinkOverlayManager", "m_LoadingText").unwrap_or("LOADING").to_string();
            text_quads(commands, root, f, &t, &info, info.color, Vec2::ZERO);
        }
        if !self.show_texts {
            return;
        }
        if let (Some(f), Some(name)) = (cfg.title.as_ref(), cfg.loc.get("MapNames", &self.map)) {
            let info = text_info(&cfg.ui, "m_MapNameTextInfo");
            // Light shadow under the dark name (`m_fMapNameShadowColor`).
            let raw = cfg.ui.get("DishonoredGame.DisBinkOverlayManager", "m_fMapNameShadowColor").unwrap_or("");
            let w = format!("({})", raw.trim_start_matches('(').trim_end_matches(')'));
            let sc = Color::srgb(num(field(&w, "R")) / 255.0, num(field(&w, "G")) / 255.0, num(field(&w, "B")) / 255.0);
            let name = name.to_uppercase();
            text_quads(commands, root, f, &name, &info, sc, Vec2::new(2.0, 2.0));
            text_quads(commands, root, f, &name, &info, info.color, Vec2::ZERO);
        }
        if let (Some(f), Some(h)) = (normal, self.hints.get(self.hint)) {
            let info = text_info(&cfg.ui, "m_HintTextInfo");
            text_quads(commands, root, f, h, &info, info.color, Vec2::ZERO);
        }
    }

    /// The level is ready: take the screen down (after a short minimum, so it is seen).
    pub fn end(&mut self) {
        self.want_end = true;
    }

    /// Take the screen down now (the launcher, once the game window has taken over).
    pub fn close(&mut self, commands: &mut Commands) {
        self.end_now(commands);
    }

    fn end_now(&mut self, commands: &mut Commands) {
        if let Some(r) = self.root.take() {
            commands.entity(r).despawn();
        }
        self.video = None;
        self.texts_root = None;
        self.active = false;
        self.want_end = false;
    }
}

/// Stream video frames, rotate hints, and close when the level is in.
pub fn update_loading_screen(mut commands: Commands, time: Res<Time>, mut ls: ResMut<LoadingScreen>, mut images: ResMut<Assets<Image>>) {
    if !ls.active {
        return;
    }
    let dt = time.delta_secs();
    ls.shown_for += dt;
    if let Some(v) = &ls.video {
        if let Some(frame) = v.frame.lock().unwrap().take() {
            if let Some(mut img) = images.get_mut(&v.image) {
                img.data = Some(frame);
            }
        }
    }
    if ls.hints.len() > 1 {
        ls.hint_t += dt;
        if ls.hint_t >= ls.hint_duration {
            ls.hint_t = 0.0;
            ls.hint = (ls.hint + 1) % ls.hints.len();
            ls.draw_texts(&mut commands);
        }
    }
    if ls.want_end && ls.shown_for >= 1.0 {
        ls.end_now(&mut commands);
    }
}
