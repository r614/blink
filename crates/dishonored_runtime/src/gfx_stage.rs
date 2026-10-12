//! A small Scaleform (GFx 3, ActionScript 2) stage: plays one of Dishonored's UI movies the way
//! the player does, so the HUD looks, moves and times exactly like the movie.
//!
//! What the movie data drives is played from the data:
//! - every sprite instance has its own playhead advanced at the movie's frame rate (30 fps),
//!   looping at the end, with the frame scripts the HUD movies use (`stop`, `play`,
//!   `gotoAndPlay(label | frame)`, `this._visible = b`, decoded by `swf::parse_frame_script`);
//! - timeline placements, moves, colour transforms and removals (`display_list`), and timeline
//!   masks (`clipDepth`);
//! - text fields (font, size, colour, alignment, word wrap, `autoSize`) measured with the game
//!   fonts' own advances (`DisFonts_SF`).
//!
//! What ActionScript does is reproduced by the widget code (`hud_widgets.rs`, ...) through the
//! same primitives the classes use: `_x/_y/_xscale/_yscale/_rotation/_alpha/_visible`,
//! `_width/_height`, `gotoAndPlay/gotoAndStop`, `attachMovie`, `createEmptyMovieClip`,
//! `removeMovieClip`, `setMask`, `text/htmlText/textColor/autoSize/wordWrap`, `textWidth`,
//! `MovieClip.tweenTo` (`gfx.motion.Tween`: one tween per clip, time based, values applied on
//! each movie frame) with the `mx.transitions.easing` curves, `setInterval` (one-shot here: every
//! HUD callback clears its interval first) and `ImgLoader` (external `img://` textures).
//!
//! Rendering: the stage is flattened every frame into textured quads (masks crop them in the
//! quad's own texture space), solid quads and text runs, drawn with Bevy UI nodes from a pool
//! (`StageView`), scaled from the 1280x720 stage to the window ("show all", centred). Not
//! reproduced: blend modes (additive glows draw as normal), filters (glow / drop shadow on text),
//! gradient fills, skew (quads keep their two edge lengths and the first edge's angle).

use crate::map_render::to_image;
use bevy::prelude::*;
use bevy::text::{LetterSpacing, LineHeight};
use dishonored_assets::swf::{ColorTransform, Fill, FrameOp, Matrix, Movie, Place, ScriptOp};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use ue3_package::ObjRef;
use ue3_package::properties::find;

pub const STAGE: Vec2 = Vec2::new(1280.0, 720.0);

/// Node handle in a [`Stage`].
pub type Id = usize;

// ---------------------------------------------------------------------------------------------
// Easing (`mx.transitions.easing`, `gfx.motion.Tween.linearEase`), normalised: b = 0, c = 1, d = 1.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ease {
    Linear,
    StrongIn,
    StrongOut,
    StrongInOut,
    BackIn,
    BackOut,
    BackInOut,
    /// `mx.transitions.easing.Regular.easeOut` (quadratic; the power wheel's info fade).
    RegularOut,
}

impl Ease {
    /// The curve at `t` in 0..1 (decoded from the movies' `mx.transitions.easing.Strong` and
    /// `.Back` classes: Strong is quintic, Back overshoots with s = 1.70158, in-out s * 1.525).
    pub fn at(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        const S: f32 = 1.70158;
        match self {
            Ease::Linear => t,
            Ease::RegularOut => t * (2.0 - t),
            Ease::StrongIn => t.powi(5),
            Ease::StrongOut => (t - 1.0).powi(5) + 1.0,
            Ease::StrongInOut => {
                let t2 = t * 2.0;
                if t2 < 1.0 {
                    0.5 * t2.powi(5)
                } else {
                    0.5 * ((t2 - 2.0).powi(5) + 2.0)
                }
            }
            Ease::BackIn => t * t * ((S + 1.0) * t - S),
            Ease::BackOut => {
                let u = t - 1.0;
                u * u * ((S + 1.0) * u + S) + 1.0
            }
            Ease::BackInOut => {
                let s = S * 1.525;
                let t2 = t * 2.0;
                if t2 < 1.0 {
                    0.5 * (t2 * t2 * ((s + 1.0) * t2 - s))
                } else {
                    let u = t2 - 2.0;
                    0.5 * (u * u * ((s + 1.0) * u + s) + 2.0)
                }
            }
        }
    }
}

/// A clip property a tween drives (AS2 units: pixels, percent, degrees).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prop {
    X,
    Y,
    XScale,
    YScale,
    Rotation,
    Alpha,
}

// ---------------------------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------------------------

/// Advance widths of a movie font (1024-unit EM), for `textWidth` / `autoSize` / word wrap.
#[derive(Clone, Debug, Default)]
pub struct FontMetrics {
    pub advances: HashMap<u16, f32>,
    pub ascent: f32,
    pub descent: f32,
    pub leading: f32,
}

impl FontMetrics {
    pub fn from_swf(f: &dishonored_assets::swf_font::SwfFont) -> Self {
        let advances = f.codes.iter().zip(f.advances.iter()).map(|(c, a)| (*c, *a as f32)).collect();
        Self { advances, ascent: f.ascent as f32, descent: f.descent as f32, leading: f.leading as f32 }
    }
    /// Width of `s` at `size` pixels (missing glyphs count as half an EM).
    pub fn width(&self, s: &str, size: f32, spacing: f32) -> f32 {
        let mut w = 0.0;
        let mut n: usize = 0;
        for c in s.chars() {
            w += self.advances.get(&(c as u32 as u16)).copied().unwrap_or(512.0) / 1024.0 * size;
            n += 1;
        }
        w + spacing * n.saturating_sub(1).max(0) as f32
    }
    /// Line height at `size` pixels (ascent + descent).
    pub fn line(&self, size: f32) -> f32 {
        ((self.ascent + self.descent) / 1024.0 * size).max(size * 0.8)
    }
}

/// The two UI typefaces of `DisFonts_SF`: `$NormalFont` ("Emerge BF") and `$TitleFont`
/// ("ChaletComprime-CologneEighty").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontKind {
    Normal,
    Title,
}

#[derive(Clone, Default)]
pub struct Fonts {
    pub normal: FontMetrics,
    pub title: FontMetrics,
}

impl Fonts {
    pub fn get(&self, k: FontKind) -> &FontMetrics {
        match k {
            FontKind::Normal => &self.normal,
            FontKind::Title => &self.title,
        }
    }
    /// Load the metrics from `DisFonts_SF` (`fonts_efigs`).
    pub fn load(content: &dishonored_assets::Content) -> Self {
        let mut out = Self::default();
        let Some(movie) = content.package("DisFonts_SF").ok().and_then(|p| movie_raw(&p, "fonts_efigs")).and_then(|r| Movie::parse(&r).ok()) else { return out };
        for f in &movie.fonts {
            if f.name.starts_with("Emerge") {
                out.normal = FontMetrics::from_swf(f);
            } else if f.name.starts_with("Chalet") {
                out.title = FontMetrics::from_swf(f);
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// Movie + assets
// ---------------------------------------------------------------------------------------------

pub fn movie_raw(pkg: &ue3_package::Package, name: &str) -> Option<Vec<u8>> {
    let i = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SwfMovie" && pkg.obj_path(ObjRef::Export(i)).ends_with(name))?;
    let h = pkg.read_object(i).ok()?;
    let (n, raw) = find(&h.properties, "RawData").and_then(|p| p.array_raw())?;
    Some(raw[..n].to_vec())
}

/// A parsed movie with what its stage needs at run time.
pub struct MovieData {
    pub movie: Movie,
    /// Font kind of each font id used by a text field.
    pub font_kind: HashMap<u16, FontKind>,
    pub fonts: Fonts,
    /// Movie frame rate.
    pub fps: f32,
    /// For a library movie: the path fragment other movies' `ImportAssets` URLs carry
    /// ("common_assets" for `../common_assets/lib.swf`).
    pub url_key: String,
}

impl MovieData {
    /// Load `movie` from `package`.
    pub fn load(content: &dishonored_assets::Content, package: &str, movie: &str, fonts: &Fonts, url_key: &str) -> Option<Self> {
        let pkg = content.package(package).ok()?;
        let m = Movie::parse(&movie_raw(&pkg, movie)?).ok()?;
        let mut d = Self::new(m, fonts.clone());
        d.url_key = url_key.to_string();
        Some(d)
    }

    pub fn new(movie: Movie, fonts: Fonts) -> Self {
        let mut font_kind = HashMap::new();
        for t in movie.edit_texts.values() {
            if let Some(id) = t.font {
                let k = match movie.font_name(id) {
                    Some(n) if n.contains("Title") || n.starts_with("Chalet") => FontKind::Title,
                    _ => FontKind::Normal,
                };
                font_kind.insert(id, k);
            }
        }
        let fps = if movie.frame_rate > 1.0 { movie.frame_rate } else { 30.0 };
        Self { movie, font_kind, fonts, fps, url_key: String::new() }
    }
}

// ---------------------------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------------------------

/// `autoSize` of a text field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoSize {
    None,
    Left,
    Right,
    Center,
}

#[derive(Clone, Debug)]
pub struct TextState {
    /// Plain text (HTML tags removed, `<br>` as newlines).
    pub text: String,
    pub color: Option<[u8; 3]>,
    pub auto_size: AutoSize,
    pub word_wrap: bool,
    /// Letter spacing (pixels) set by a `TextFormat` (the subtitles use 0.5).
    pub letter_spacing: f32,
}

/// An external image (`img://Package.Texture`) loaded into a clip by `ImgLoader`.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageRef {
    /// "Package.Texture".
    pub path: String,
    /// Pixel size of the texture.
    pub size: Vec2,
}

#[derive(Clone, Debug)]
pub struct StageNode {
    /// Movie the character belongs to (0: the stage's movie; others: imported libraries).
    pub src: usize,
    pub character: u16,
    pub name: Option<String>,
    pub parent: Option<Id>,
    /// Timeline depth, or the depth given to `attachMovie` / `createEmptyMovieClip`.
    pub depth: i32,
    /// Created by script (kept by timeline gotos).
    pub dynamic: bool,
    pub children: BTreeMap<i32, Id>,
    pub matrix: Matrix,
    pub color: ColorTransform,
    /// Script touched the transform / colour: the timeline no longer moves it.
    pub scripted: bool,
    pub visible: bool,
    pub frame: usize,
    pub playing: bool,
    pub clip_depth: Option<i32>,
    /// `setMask`: the clip masking this one.
    pub mask: Option<Id>,
    pub is_mask: bool,
    pub text: Option<TextState>,
    pub image: Option<ImageRef>,
}

#[derive(Clone, Debug)]
struct Tween {
    node: Id,
    start: f64,
    duration: f32,
    ease: Ease,
    props: Vec<(Prop, f32, f32)>,
    key: Option<u64>,
}

#[derive(Clone, Debug)]
struct Timer {
    id: u32,
    due: f64,
    key: u64,
}

/// What the stage reports to the widget code.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// An interval set with [`Stage::set_interval`] fired.
    Timer(u64),
    /// A tween with a completion key finished (`onTweenComplete`).
    TweenDone(u64),
}

/// One playing movie.
pub struct Stage {
    pub data: Arc<MovieData>,
    /// The stage's movie (0) and the libraries it imports from.
    pub movies: Vec<Arc<MovieData>>,
    nodes: Vec<Option<StageNode>>,
    pub root: Id,
    /// Movie clock (seconds): `getTimer()` / 1000.
    pub time: f64,
    next_frame: f64,
    tweens: Vec<Tween>,
    timers: Vec<Timer>,
    next_timer: u32,
    events: VecDeque<Event>,
    /// Display lists per (movie, sprite, frame), cached.
    dl_cache: HashMap<(usize, u16, usize), Arc<Vec<Place>>>,
}

fn decompose(m: &Matrix) -> (f32, f32, f32) {
    let xs = (m.a * m.a + m.b * m.b).sqrt();
    let ys = (m.c * m.c + m.d * m.d).sqrt();
    let rot = m.b.atan2(m.a).to_degrees();
    (xs, ys, rot)
}

/// Strip the HTML of an `htmlText` (tags, entities; `<br>` and paragraph ends as newlines).
pub fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let b = s.as_bytes();
    while i < b.len() {
        if b[i] == b'<' {
            let end = s[i..].find('>').map(|e| i + e).unwrap_or(s.len() - 1);
            let tag = s[i + 1..end].trim().to_ascii_lowercase();
            if tag.starts_with("br") || tag == "/p" {
                out.push('\n');
            }
            i = end + 1;
            continue;
        }
        if b[i] == b'&' {
            let rest = &s[i..];
            let ents = [("&amp;", '&'), ("&lt;", '<'), ("&gt;", '>'), ("&quot;", '"'), ("&apos;", '\''), ("&nbsp;", ' ')];
            if let Some((e, c)) = ents.iter().find(|(e, _)| rest.starts_with(e)) {
                out.push(*c);
                i += e.len();
                continue;
            }
        }
        let ch = s[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    // A trailing paragraph end is not a line.
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

impl Stage {
    /// A stage for `data`, with the library movies its `ImportAssets` refer to.
    pub fn new(data: Arc<MovieData>, libs: Vec<Arc<MovieData>>) -> Self {
        let frame_dt = 1.0 / data.fps as f64;
        let mut movies = vec![data.clone()];
        movies.extend(libs);
        let mut s = Self { data, movies, nodes: Vec::new(), root: 0, time: 0.0, next_frame: frame_dt, tweens: Vec::new(), timers: Vec::new(), next_timer: 1, events: VecDeque::new(), dl_cache: HashMap::new() };
        let root = s.alloc(StageNode {
            src: 0,
            character: 0,
            name: Some("_root".into()),
            parent: None,
            depth: 0,
            dynamic: false,
            children: BTreeMap::new(),
            matrix: Matrix::default(),
            color: ColorTransform::default(),
            scripted: false,
            visible: true,
            frame: 0,
            playing: false,
            clip_depth: None,
            mask: None,
            is_mask: false,
            text: None,
            image: None,
        });
        s.root = root;
        // The root timeline: frame 0's placements (its scripts are the movie's debug harness).
        let places = s.data.movie.display_list(&s.data.movie.root, 0);
        for p in places {
            s.place_child(root, 0, &p);
        }
        s
    }

    fn mv(&self, src: usize) -> &Movie {
        &self.movies[src.min(self.movies.len() - 1)].movie
    }

    /// Movie and character an instance of `character` (of movie `src`) really is: imported
    /// characters resolve to the library movie's export.
    fn resolve(&self, src: usize, character: u16) -> (usize, u16) {
        if let Some((url, name)) = self.mv(src).imports.get(&character) {
            let url = url.to_ascii_lowercase().replace('\\', "/");
            for (i, m) in self.movies.iter().enumerate() {
                if i != src && !m.url_key.is_empty() && url.contains(&m.url_key) {
                    if let Some(id) = m.movie.exports.get(name) {
                        return (i, *id);
                    }
                }
            }
        }
        (src, character)
    }

    /// Movie of a node.
    pub fn src_of(&self, id: Id) -> usize {
        self.node(id).map(|n| n.src).unwrap_or(0)
    }

    fn alloc(&mut self, n: StageNode) -> Id {
        self.nodes.push(Some(n));
        self.nodes.len() - 1
    }

    pub fn node(&self, id: Id) -> Option<&StageNode> {
        self.nodes.get(id).and_then(|n| n.as_ref())
    }
    fn node_mut(&mut self, id: Id) -> Option<&mut StageNode> {
        self.nodes.get_mut(id).and_then(|n| n.as_mut())
    }
    pub fn alive(&self, id: Id) -> bool {
        self.node(id).is_some()
    }

    fn frames_of(&self, src: usize, character: u16) -> usize {
        self.mv(src).sprites.get(&character).map(|s| s.frames.len()).unwrap_or(1).max(1)
    }

    fn display_list(&mut self, src: usize, character: u16, frame: usize) -> Arc<Vec<Place>> {
        if let Some(d) = self.dl_cache.get(&(src, character, frame)) {
            return d.clone();
        }
        let list = match self.mv(src).sprites.get(&character) {
            Some(sp) => self.mv(src).display_list(sp, frame),
            None => Vec::new(),
        };
        let a = Arc::new(list);
        self.dl_cache.insert((src, character, frame), a.clone());
        a
    }

    /// Instantiate a character (sprite, shape or text) of movie `src` as a child of `parent`.
    #[allow(clippy::too_many_arguments)]
    fn instantiate(&mut self, parent: Id, src: usize, character: u16, depth: i32, name: Option<String>, matrix: Matrix, color: ColorTransform, dynamic: bool, clip_depth: Option<i32>) -> Id {
        let (src, character) = self.resolve(src, character);
        let text = self.mv(src).edit_texts.get(&character).map(|t| TextState {
            text: t.initial.as_deref().map(|s| if t.html { strip_html(s) } else { s.to_string() }).unwrap_or_default(),
            color: None,
            auto_size: if t.auto_size { AutoSize::Left } else { AutoSize::None },
            word_wrap: t.word_wrap,
            // The field's default format keeps the authored `letterSpacing`.
            letter_spacing: t
                .initial
                .as_deref()
                .and_then(|s| s.split("letterSpacing=\"").nth(1))
                .and_then(|s| s.split('"').next())
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0),
        });
        let playing = self.frames_of(src, character) > 1;
        let id = self.alloc(StageNode {
            src,
            character,
            name,
            parent: Some(parent),
            depth,
            dynamic,
            children: BTreeMap::new(),
            matrix,
            color,
            scripted: false,
            visible: true,
            frame: 0,
            playing,
            clip_depth,
            mask: None,
            is_mask: false,
            text,
            image: None,
        });
        let old = self.node_mut(parent).and_then(|p| p.children.insert(depth, id));
        if let Some(old) = old.filter(|o| *o != id) {
            self.free(old);
        }
        if self.mv(src).sprites.contains_key(&character) {
            let places = self.display_list(src, character, 0);
            for p in places.iter() {
                self.place_child(id, src, p);
            }
            self.run_scripts(id, 0, 0);
        }
        id
    }

    fn place_child(&mut self, parent: Id, src: usize, p: &Place) -> Option<Id> {
        let ch = p.character?;
        Some(self.instantiate(parent, src, ch, p.depth as i32, p.name.clone(), p.matrix.unwrap_or_default(), p.color.unwrap_or_default(), false, p.clip_depth.map(|d| d as i32)))
    }

    /// Remove a node and its subtree (no unlinking from the parent).
    fn free(&mut self, id: Id) {
        let kids: Vec<Id> = self.node(id).map(|n| n.children.values().copied().collect()).unwrap_or_default();
        for k in kids {
            self.free(k);
        }
        self.tweens.retain(|t| t.node != id);
        if let Some(slot) = self.nodes.get_mut(id) {
            *slot = None;
        }
    }

    /// `removeMovieClip`.
    pub fn remove(&mut self, id: Id) {
        let Some(n) = self.node(id) else { return };
        let (parent, depth) = (n.parent, n.depth);
        if let Some(p) = parent.and_then(|p| self.node_mut(p)) {
            if p.children.get(&depth) == Some(&id) {
                p.children.remove(&depth);
            }
        }
        // Masked clips forget a removed mask.
        for n in self.nodes.iter_mut().flatten() {
            if n.mask == Some(id) {
                n.mask = None;
            }
        }
        self.free(id);
    }

    /// Bring a sprite instance's timeline children to frame `frame` (keeps instances whose depth
    /// and character match; script-created children are left alone).
    fn reconcile(&mut self, id: Id, frame: usize) {
        let Some((src, ch)) = self.node(id).map(|n| (n.src, n.character)) else { return };
        let list = self.display_list(src, ch, frame);
        let existing: Vec<(i32, Id)> = self.node(id).map(|n| n.children.iter().map(|(d, i)| (*d, *i)).collect()).unwrap_or_default();
        for (d, cid) in existing {
            let Some(c) = self.node(cid) else { continue };
            if c.dynamic {
                continue;
            }
            let same = |p: &Place| p.character.map(|pc| self.resolve(src, pc)) == Some((c.src, c.character));
            match list.iter().find(|p| p.depth as i32 == d) {
                Some(p) if same(p) => {
                    let scripted = c.scripted;
                    let (m, col, name, cd) = (p.matrix, p.color, p.name.clone(), p.clip_depth);
                    if let Some(c) = self.node_mut(cid) {
                        if !scripted {
                            c.matrix = m.unwrap_or_default();
                            c.color = col.unwrap_or_default();
                        }
                        if name.is_some() {
                            c.name = name;
                        }
                        c.clip_depth = cd.map(|d| d as i32);
                    }
                }
                _ => {
                    if let Some(n) = self.node_mut(id) {
                        n.children.remove(&d);
                    }
                    self.free(cid);
                }
            }
        }
        for p in list.iter() {
            let have = self.node(id).is_some_and(|n| n.children.contains_key(&(p.depth as i32)));
            if !have {
                self.place_child(id, src, p);
            }
        }
    }

    fn run_scripts(&mut self, id: Id, frame: usize, depth: u32) {
        if depth > 8 {
            return;
        }
        let Some((src, ch)) = self.node(id).map(|n| (n.src, n.character)) else { return };
        let ops: Vec<ScriptOp> = match self.mv(src).sprites.get(&ch).and_then(|s| s.frames.get(frame)) {
            Some(f) => f
                .iter()
                .filter_map(|op| match op {
                    FrameOp::Script(s) => Some(s.clone()),
                    _ => None,
                })
                .flatten()
                .collect(),
            None => return,
        };
        let mut jump: Option<usize> = None;
        for op in ops {
            match op {
                ScriptOp::Stop => {
                    if let Some(n) = self.node_mut(id) {
                        n.playing = false;
                    }
                }
                ScriptOp::Play => {
                    if let Some(n) = self.node_mut(id) {
                        n.playing = true;
                    }
                }
                ScriptOp::GotoFrame(f) => jump = Some(f as usize),
                ScriptOp::GotoLabel(l) => {
                    if let Some(f) = self.mv(src).sprites.get(&ch).and_then(|s| Movie::label_frame(s, &l)) {
                        jump = Some(f);
                    }
                }
                ScriptOp::SetVisible(v) => {
                    if let Some(n) = self.node_mut(id) {
                        n.visible = v;
                    }
                }
                ScriptOp::Other => {}
            }
        }
        if let Some(f) = jump {
            self.goto_frame(id, f, depth + 1);
        }
    }

    fn goto_frame(&mut self, id: Id, frame: usize, depth: u32) {
        let Some((src, ch)) = self.node(id).map(|n| (n.src, n.character)) else { return };
        let frame = frame.min(self.frames_of(src, ch) - 1);
        if let Some(n) = self.node_mut(id) {
            n.frame = frame;
        }
        self.reconcile(id, frame);
        self.run_scripts(id, frame, depth);
    }

    fn label_or_frame(&self, id: Id, target: &str) -> Option<usize> {
        let n = self.node(id)?;
        let sp = self.mv(n.src).sprites.get(&n.character)?;
        if let Ok(n) = target.parse::<usize>() {
            return Some(n.saturating_sub(1));
        }
        Movie::label_frame(sp, target)
    }

    /// `gotoAndPlay(label | 1-based frame)`.
    pub fn goto_and_play(&mut self, id: Id, target: &str) {
        let Some(f) = self.label_or_frame(id, target) else { return };
        if let Some(n) = self.node_mut(id) {
            n.playing = true;
        }
        self.goto_frame(id, f, 0);
    }
    /// `gotoAndStop(label | 1-based frame)`.
    pub fn goto_and_stop(&mut self, id: Id, target: &str) {
        let Some(f) = self.label_or_frame(id, target) else { return };
        if let Some(n) = self.node_mut(id) {
            n.playing = false;
        }
        self.goto_frame(id, f, 0);
        if let Some(n) = self.node_mut(id) {
            n.playing = false;
        }
    }
    pub fn stop(&mut self, id: Id) {
        if let Some(n) = self.node_mut(id) {
            n.playing = false;
        }
    }
    pub fn play(&mut self, id: Id) {
        if let Some(n) = self.node_mut(id) {
            n.playing = true;
        }
    }
    /// `_currentframe` (1-based).
    pub fn current_frame(&self, id: Id) -> usize {
        self.node(id).map(|n| n.frame + 1).unwrap_or(0)
    }
    pub fn total_frames(&self, id: Id) -> usize {
        self.node(id).map(|n| self.frames_of(n.src, n.character)).unwrap_or(0)
    }
    pub fn has_label(&self, id: Id, label: &str) -> bool {
        self.label_or_frame(id, label).is_some() && label.parse::<usize>().is_err()
    }

    // --- tree -----------------------------------------------------------------------------

    /// Child by instance name.
    pub fn child(&self, id: Id, name: &str) -> Option<Id> {
        let n = self.node(id)?;
        n.children.values().copied().find(|c| self.node(*c).and_then(|c| c.name.as_deref()) == Some(name))
    }
    /// Descendant by a '/'- or '.'-separated instance path ("a/b/c").
    pub fn find(&self, id: Id, path: &str) -> Option<Id> {
        let mut cur = id;
        for part in path.split(['/', '.']).filter(|p| !p.is_empty()) {
            cur = self.child(cur, part)?;
        }
        Some(cur)
    }
    pub fn parent(&self, id: Id) -> Option<Id> {
        self.node(id)?.parent
    }
    pub fn children(&self, id: Id) -> Vec<Id> {
        self.node(id).map(|n| n.children.values().copied().collect()).unwrap_or_default()
    }
    /// `getNextHighestDepth()`.
    pub fn next_depth(&self, id: Id) -> i32 {
        self.node(id).and_then(|n| n.children.keys().next_back().copied()).map(|d| d + 1).unwrap_or(0).max(0)
    }
    /// `attachMovie(export, name, depth)`: None when the export does not exist.
    pub fn attach(&mut self, parent: Id, export: &str, name: &str, depth: i32) -> Option<Id> {
        // The parent's movie first (a library's own clips attach its exports), then the others.
        let ps = self.src_of(parent);
        let order = std::iter::once(ps).chain((0..self.movies.len()).filter(|i| *i != ps));
        let (src, ch) = order.into_iter().find_map(|i| self.movies[i].movie.exports.get(export).map(|c| (i, *c)))?;
        Some(self.instantiate(parent, src, ch, depth, Some(name.to_string()), Matrix::default(), ColorTransform::default(), true, None))
    }
    /// `createEmptyMovieClip(name, depth)`.
    pub fn create_empty(&mut self, parent: Id, name: &str, depth: i32) -> Id {
        self.instantiate(parent, 0, 0, depth, Some(name.to_string()), Matrix::default(), ColorTransform::default(), true, None)
    }
    /// `setMask(mask)`.
    pub fn set_mask(&mut self, id: Id, mask: Option<Id>) {
        if let Some(m) = mask.and_then(|m| self.node_mut(m)) {
            m.is_mask = true;
        }
        if let Some(n) = self.node_mut(id) {
            n.mask = mask;
        }
    }

    // --- properties -----------------------------------------------------------------------

    pub fn get(&self, id: Id, p: Prop) -> f32 {
        let Some(n) = self.node(id) else { return 0.0 };
        let (xs, ys, rot) = decompose(&n.matrix);
        match p {
            Prop::X => n.matrix.tx / 20.0,
            Prop::Y => n.matrix.ty / 20.0,
            Prop::XScale => xs * 100.0,
            Prop::YScale => ys * 100.0,
            Prop::Rotation => rot,
            Prop::Alpha => n.color.mul[3] * 100.0,
        }
    }

    pub fn set(&mut self, id: Id, p: Prop, v: f32) {
        let Some(n) = self.node_mut(id) else { return };
        n.scripted = true;
        let m = &mut n.matrix;
        match p {
            Prop::X => m.tx = v * 20.0,
            Prop::Y => m.ty = v * 20.0,
            Prop::XScale => {
                let (xs, _, rot) = decompose(m);
                let s = v / 100.0;
                if xs > 1e-6 {
                    m.a *= s / xs;
                    m.b *= s / xs;
                } else {
                    let (sn, cs) = rot.to_radians().sin_cos();
                    m.a = cs * s;
                    m.b = sn * s;
                }
            }
            Prop::YScale => {
                let (_, ys, rot) = decompose(m);
                let s = v / 100.0;
                if ys > 1e-6 {
                    m.c *= s / ys;
                    m.d *= s / ys;
                } else {
                    let (sn, cs) = rot.to_radians().sin_cos();
                    m.c = -sn * s;
                    m.d = cs * s;
                }
            }
            Prop::Rotation => {
                let (xs, ys, _) = decompose(m);
                let (sn, cs) = v.to_radians().sin_cos();
                m.a = cs * xs;
                m.b = sn * xs;
                m.c = -sn * ys;
                m.d = cs * ys;
            }
            Prop::Alpha => n.color.mul[3] = v / 100.0,
        }
    }

    pub fn set_visible(&mut self, id: Id, v: bool) {
        if let Some(n) = self.node_mut(id) {
            n.visible = v;
        }
    }
    pub fn visible(&self, id: Id) -> bool {
        self.node(id).is_some_and(|n| n.visible)
    }
    /// Colour offset (`SetColorTransform.SetOffsetColor_Custom`, 0..255 per channel) keeping the
    /// multipliers.
    pub fn set_color_offset(&mut self, id: Id, rgb: [f32; 3]) {
        if let Some(n) = self.node_mut(id) {
            n.scripted = true;
            for i in 0..3 {
                n.color.add[i] = rgb[i] / 255.0;
            }
        }
    }
    /// Colour multipliers and offsets (`Color.setTransform`).
    pub fn set_color(&mut self, id: Id, c: ColorTransform) {
        if let Some(n) = self.node_mut(id) {
            n.scripted = true;
            let a = n.color.mul[3];
            n.color = c;
            n.color.mul[3] = a;
        }
    }

    // --- text -----------------------------------------------------------------------------

    /// `text = s`.
    pub fn set_text(&mut self, id: Id, s: &str) {
        if let Some(t) = self.node_mut(id).and_then(|n| n.text.as_mut()) {
            t.text = s.to_string();
        }
    }
    /// `htmlText = s`.
    pub fn set_html(&mut self, id: Id, s: &str) {
        self.set_text(id, &strip_html(s));
    }
    pub fn text(&self, id: Id) -> String {
        self.node(id).and_then(|n| n.text.as_ref()).map(|t| t.text.clone()).unwrap_or_default()
    }
    pub fn set_text_color(&mut self, id: Id, rgb: u32) {
        if let Some(t) = self.node_mut(id).and_then(|n| n.text.as_mut()) {
            t.color = Some([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8]);
        }
    }
    pub fn set_auto_size(&mut self, id: Id, a: AutoSize) {
        if let Some(t) = self.node_mut(id).and_then(|n| n.text.as_mut()) {
            t.auto_size = a;
        }
    }
    pub fn set_word_wrap(&mut self, id: Id, w: bool) {
        if let Some(t) = self.node_mut(id).and_then(|n| n.text.as_mut()) {
            t.word_wrap = w;
        }
    }
    pub fn set_letter_spacing(&mut self, id: Id, px: f32) {
        if let Some(t) = self.node_mut(id).and_then(|n| n.text.as_mut()) {
            t.letter_spacing = px;
        }
    }

    /// Lines of a text field as laid out (field box in field space, pixels) and its font.
    pub fn text_layout(&self, id: Id) -> Option<TextLayoutInfo> {
        let n = self.node(id)?;
        let t = n.text.as_ref()?;
        let md = &self.movies[n.src.min(self.movies.len() - 1)];
        let et = md.movie.edit_texts.get(&n.character)?;
        let kind = et.font.and_then(|f| md.font_kind.get(&f)).copied().unwrap_or(FontKind::Normal);
        let fm = md.fonts.get(kind);
        let size = et.height.max(1.0);
        let b = et.bounds;
        let (x0, y0, x1, y1) = (b.xmin / 20.0, b.ymin / 20.0, b.xmax / 20.0, b.ymax / 20.0);
        let inner = (x1 - x0 - 4.0 - et.margins[0] - et.margins[1]).max(1.0);
        let mut lines: Vec<String> = Vec::new();
        for para in t.text.split('\n') {
            if !t.word_wrap {
                lines.push(para.to_string());
                continue;
            }
            // Greedy wrap at spaces (Flash breaks at spaces and keeps long words whole).
            let mut cur = String::new();
            for word in para.split(' ') {
                let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
                if !cur.is_empty() && fm.width(&cand, size, t.letter_spacing) > inner {
                    lines.push(std::mem::take(&mut cur));
                    cur = word.to_string();
                } else {
                    cur = cand;
                }
            }
            lines.push(cur);
        }
        let text_w = lines.iter().map(|l| fm.width(l, size, t.letter_spacing)).fold(0.0, f32::max);
        let line_h = fm.line(size) + et.leading;
        let text_h = if t.text.is_empty() { 0.0 } else { line_h * lines.len() as f32 - et.leading };
        // autoSize: the box hugs the text (width only when not wrapping), anchored on the left,
        // right or centre of the authored box.
        let (bx0, bx1) = match (t.auto_size, t.word_wrap) {
            (AutoSize::None, _) | (_, true) => (x0, x1),
            (AutoSize::Left, false) => (x0, x0 + text_w + 4.0),
            (AutoSize::Right, false) => (x1 - text_w - 4.0, x1),
            (AutoSize::Center, false) => ((x0 + x1) * 0.5 - text_w * 0.5 - 2.0, (x0 + x1) * 0.5 + text_w * 0.5 + 2.0),
        };
        let by1 = if t.auto_size != AutoSize::None { y0 + text_h + 4.0 } else { y1 };
        let align = match et.align {
            1 => Justify::Right,
            2 => Justify::Center,
            _ => Justify::Left,
        };
        let c = t.color.map(|c| [c[0], c[1], c[2], 255]).or(et.color).unwrap_or([255, 255, 255, 255]);
        Some(TextLayoutInfo { lines, rect: Rect::new(bx0, y0, bx1, by1), size, kind, align, color: c, text_w, text_h, line_h, margins: et.margins, letter_spacing: t.letter_spacing })
    }

    /// `textWidth`.
    pub fn text_width(&self, id: Id) -> f32 {
        self.text_layout(id).map(|l| l.text_w).unwrap_or(0.0)
    }
    /// `textHeight`.
    pub fn text_height(&self, id: Id) -> f32 {
        self.text_layout(id).map(|l| l.text_h).unwrap_or(0.0)
    }

    // --- images ---------------------------------------------------------------------------

    /// Show an external texture in a clip at its local origin (the `imgContainer` of an
    /// `ImgLoader`); None removes it.
    pub fn set_image(&mut self, id: Id, img: Option<ImageRef>) {
        if let Some(n) = self.node_mut(id) {
            n.image = img;
        }
    }

    /// `new _common.ImgLoader(img, mc, bDisableAutoFit, hAlign, vAlign)`: an `imgContainer`
    /// child of `mc` showing the texture, centred on `mc`'s origin (`hAlign` / `vAlign`
    /// "center", the default). `auto_fit` is `bDisableAutoFit === false` (the class tests
    /// `!(b || b == null)`, so a missing argument does not resize): then an image wider or
    /// taller than `mc` was is scaled by `maxH / h` (the compiled `resizeImg` falls through to
    /// the height ratio in both branches). None removes the image.
    pub fn img_loader(&mut self, mc: Id, img: Option<ImageRef>, auto_fit: bool, center: bool) {
        let max = self.bounds_in_parent(mc).map(|r| r.size()).unwrap_or(Vec2::ZERO);
        // Size in `mc`'s own space: undo its scale.
        let (xs, ys) = (self.get(mc, Prop::XScale).max(0.01) / 100.0, self.get(mc, Prop::YScale).max(0.01) / 100.0);
        let (max_w, max_h) = (max.x / xs, max.y / ys);
        if let Some(old) = self.child(mc, "imgContainer") {
            self.remove(old);
        }
        let Some(img) = img else { return };
        let depth = self.next_depth(mc);
        let c = self.create_empty(mc, "imgContainer", depth);
        let (w, h) = (img.size.x, img.size.y);
        let s = if auto_fit && (w > max_w || h > max_h) && h > 0.0 { max_h / h } else { 1.0 };
        self.set_image(c, Some(img));
        self.set(c, Prop::XScale, s * 100.0);
        self.set(c, Prop::YScale, s * 100.0);
        if center {
            self.set(c, Prop::X, -0.5 * w * s);
            self.set(c, Prop::Y, -0.5 * h * s);
        }
    }

    // --- geometry -------------------------------------------------------------------------

    /// Bounds of a node's content in its own space (pixels): shapes, text boxes, images and
    /// children (masks excluded).
    pub fn local_bounds(&self, id: Id) -> Option<Rect> {
        let n = self.node(id)?;
        let mut r: Option<Rect> = None;
        let mut add = |rr: Rect| {
            r = Some(match r {
                Some(a) => a.union(rr),
                None => rr,
            });
        };
        if let Some(sh) = self.mv(n.src).shapes.get(&n.character) {
            let b = sh.bounds;
            add(Rect::new(b.xmin / 20.0, b.ymin / 20.0, b.xmax / 20.0, b.ymax / 20.0));
        }
        if n.text.is_some() {
            if let Some(l) = self.text_layout(id) {
                add(l.rect);
            }
        }
        if let Some(img) = &n.image {
            add(Rect::new(0.0, 0.0, img.size.x, img.size.y));
        }
        for c in n.children.values() {
            let Some(cn) = self.node(*c) else { continue };
            if cn.is_mask || cn.clip_depth.is_some() {
                continue;
            }
            if let Some(cb) = self.local_bounds(*c) {
                add(transform_rect(&cn.matrix, cb));
            }
        }
        r
    }

    /// Bounds in the parent's space (what `_width` / `_height` measure).
    pub fn bounds_in_parent(&self, id: Id) -> Option<Rect> {
        let n = self.node(id)?;
        self.local_bounds(id).map(|b| transform_rect(&n.matrix, b))
    }
    pub fn width(&self, id: Id) -> f32 {
        self.bounds_in_parent(id).map(|r| r.width()).unwrap_or(0.0)
    }
    pub fn height(&self, id: Id) -> f32 {
        self.bounds_in_parent(id).map(|r| r.height()).unwrap_or(0.0)
    }
    /// `_width = w` (scales x to the width; unrotated clips).
    pub fn set_width(&mut self, id: Id, w: f32) {
        let Some(lb) = self.local_bounds(id) else { return };
        if lb.width() > 1e-4 {
            self.set(id, Prop::XScale, w / lb.width() * 100.0);
        }
    }
    pub fn set_height(&mut self, id: Id, h: f32) {
        let Some(lb) = self.local_bounds(id) else { return };
        if lb.height() > 1e-4 {
            self.set(id, Prop::YScale, h / lb.height() * 100.0);
        }
    }

    /// Node -> stage matrix.
    pub fn world_matrix(&self, id: Id) -> Matrix {
        let mut chain = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            let Some(n) = self.node(c) else { break };
            chain.push(n.matrix);
            cur = n.parent;
        }
        let mut m = Matrix::default();
        for cm in chain.iter().rev() {
            m = m.mul(cm);
        }
        m
    }

    // --- tweens and intervals -------------------------------------------------------------

    /// `mc.tweenTo(duration, props, ease)`: replaces the clip's running tween (the Tween class
    /// keeps one set of tween properties per clip). `key` is reported as
    /// [`Event::TweenDone`] when it completes (`onTweenComplete`).
    pub fn tween_to(&mut self, id: Id, duration: f32, props: &[(Prop, f32)], ease: Ease, key: Option<u64>) {
        if !self.alive(id) {
            return;
        }
        self.tweens.retain(|t| t.node != id);
        let props = props.iter().map(|(p, v)| (*p, self.get(id, *p), *v)).collect();
        self.tweens.push(Tween { node: id, start: self.time, duration, ease, props, key });
    }
    /// `mc.tweenFrom(duration, props, ease)`: jump to `props` and tween back to the values the
    /// clip had.
    pub fn tween_from(&mut self, id: Id, duration: f32, props: &[(Prop, f32)], ease: Ease, key: Option<u64>) {
        if !self.alive(id) {
            return;
        }
        let ends: Vec<(Prop, f32)> = props.iter().map(|(p, _)| (*p, self.get(id, *p))).collect();
        for (p, v) in props {
            self.set(id, *p, *v);
        }
        self.tween_to(id, duration, &ends, ease, key);
    }

    /// `tweenEnd(jumpToEnd)`.
    pub fn tween_end(&mut self, id: Id, jump: bool) {
        let mut ended = Vec::new();
        self.tweens.retain(|t| {
            if t.node == id {
                ended.push(t.clone());
                false
            } else {
                true
            }
        });
        if jump {
            for t in ended {
                for (p, _, e) in t.props {
                    self.set(id, p, e);
                }
            }
        }
    }
    pub fn tweening(&self, id: Id) -> bool {
        self.tweens.iter().any(|t| t.node == id)
    }

    /// `setInterval` (one-shot: see the module notes). Returns the interval id.
    pub fn set_interval(&mut self, ms: f32, key: u64) -> u32 {
        let id = self.next_timer;
        self.next_timer = self.next_timer.wrapping_add(1).max(1);
        self.timers.push(Timer { id, due: self.time + ms as f64 / 1000.0, key });
        id
    }
    /// `clearInterval`.
    pub fn clear_interval(&mut self, id: u32) {
        self.timers.retain(|t| t.id != id);
    }
    /// Clear every interval with this key.
    pub fn clear_key(&mut self, key: u64) {
        self.timers.retain(|t| t.key != key);
    }

    /// Advance the movie clock up to `until` and return the next event on the way (None when
    /// the clock reached `until`). Movie frames (timelines, then tweens: `onEnterFrame`) run
    /// at the movie's frame rate; intervals fire at their due time.
    pub fn poll(&mut self, until: f64) -> Option<Event> {
        loop {
            if let Some(e) = self.events.pop_front() {
                return Some(e);
            }
            let next_timer = self.timers.iter().min_by(|a, b| a.due.total_cmp(&b.due)).map(|t| (t.due, t.id));
            let next_frame = self.next_frame;
            match next_timer {
                Some((due, tid)) if due <= until && due <= next_frame => {
                    self.time = self.time.max(due);
                    if let Some(i) = self.timers.iter().position(|t| t.id == tid) {
                        let t = self.timers.remove(i);
                        self.events.push_back(Event::Timer(t.key));
                    }
                }
                _ if next_frame <= until => {
                    self.time = self.time.max(next_frame);
                    self.next_frame += 1.0 / self.data.fps as f64;
                    self.advance_frame();
                }
                _ => {
                    self.time = self.time.max(until);
                    return None;
                }
            }
        }
    }

    fn advance_frame(&mut self) {
        let root = self.root;
        self.advance_node(root);
        // onEnterFrame: tweens.
        let now = self.time;
        let tweens = std::mem::take(&mut self.tweens);
        let mut keep = Vec::with_capacity(tweens.len());
        for t in tweens {
            if !self.alive(t.node) {
                continue;
            }
            let r = if t.duration > 0.0 { ((now - t.start) / t.duration as f64) as f32 } else { 1.0 };
            if r >= 1.0 {
                for (p, _, e) in &t.props {
                    self.set(t.node, *p, *e);
                }
                if let Some(k) = t.key {
                    self.events.push_back(Event::TweenDone(k));
                }
            } else {
                let k = t.ease.at(r.max(0.0));
                for (p, s, e) in &t.props {
                    self.set(t.node, *p, s + (e - s) * k);
                }
                keep.push(t);
            }
        }
        // Tweens started by the handlers above keep their own entries.
        keep.append(&mut self.tweens);
        self.tweens = keep;
    }

    fn advance_node(&mut self, id: Id) {
        let kids = self.children(id);
        for k in kids {
            if self.alive(k) {
                self.advance_node(k);
            }
        }
        let Some(n) = self.node(id) else { return };
        if !n.playing || id == self.root {
            return;
        }
        let total = self.frames_of(n.src, n.character);
        if total <= 1 {
            return;
        }
        let next = if n.frame + 1 >= total { 0 } else { n.frame + 1 };
        self.goto_frame(id, next, 0);
    }

    // --- rendering ------------------------------------------------------------------------

    /// Flatten the visible stage into draw items (stage pixels).
    pub fn draw(&self, out: &mut Vec<Draw>) {
        out.clear();
        let mut masks = Vec::new();
        self.draw_node(self.root, Matrix::default(), ColorTransform::default(), &mut masks, out);
    }

    fn mask_quad(&self, mask: Id) -> Option<[Vec2; 4]> {
        let b = self.local_bounds(mask)?;
        let m = self.world_matrix(mask);
        let p = |x: f32, y: f32| {
            let (px, py) = m.apply(x * 20.0, y * 20.0);
            Vec2::new(px, py) / 20.0
        };
        Some([p(b.min.x, b.min.y), p(b.max.x, b.min.y), p(b.max.x, b.max.y), p(b.min.x, b.max.y)])
    }

    fn draw_node(&self, id: Id, pm: Matrix, pc: ColorTransform, masks: &mut Vec<[Vec2; 4]>, out: &mut Vec<Draw>) {
        let Some(n) = self.node(id) else { return };
        if !n.visible || n.is_mask {
            return;
        }
        let m = pm.mul(&n.matrix);
        let c = ColorTransform { mul: std::array::from_fn(|i| pc.mul[i] * n.color.mul[i]), add: std::array::from_fn(|i| pc.add[i] + n.color.add[i] * pc.mul[i]) };
        if c.mul[3] <= 0.002 && c.add[3] <= 0.0 {
            return;
        }
        let pushed_mask = match n.mask.and_then(|mk| self.mask_quad(mk)) {
            Some(q) => {
                masks.push(q);
                true
            }
            None => false,
        };
        let rgba = |base: [f32; 4]| -> [f32; 4] { std::array::from_fn(|i| (base[i] * c.mul[i] + c.add[i]).clamp(0.0, 1.0)) };
        // Shape fills.
        let mv = self.mv(n.src);
        if let Some(sh) = mv.shapes.get(&n.character) {
            let b = sh.bounds;
            let corners = [(b.xmin, b.ymin), (b.xmax, b.ymin), (b.xmax, b.ymax), (b.xmin, b.ymax)].map(|(x, y)| {
                let (px, py) = m.apply(x, y);
                Vec2::new(px, py) / 20.0
            });
            // Solid and gradient fills: the shape's raster over the extent it covers.
            if dishonored_assets::swf_shape::has_vector_fill(sh) {
                let (w, h, o, sc) = dishonored_assets::swf_shape::raster_dims(sh);
                let (x1, y1) = (o.0 + w as f32 / sc, o.1 + h as f32 / sc);
                let rc = [(o.0, o.1), (x1, o.1), (x1, y1), (o.0, y1)].map(|(x, y)| {
                    let (px, py) = m.apply(x, y);
                    Vec2::new(px, py) / 20.0
                });
                if let Some((src, dst)) = clip_quad([0.0, 0.0, w as f32, h as f32], rc, masks.as_slice()) {
                    out.push(Draw::Quad { tex: TexKey::Shape(mv.key, n.character), src, dst, rgba: rgba([1.0; 4]) });
                }
            }
            for fill in &sh.fills {
                match fill {
                    Fill::Bitmap { bitmap, matrix, .. } => {
                        let tex = match (mv.sub_images.get(bitmap), mv.images.get(&(*bitmap as u32))) {
                            (None, Some(im)) => Some((im.file_name.clone(), Vec2::ZERO)),
                            (Some((img, x1, y1, _, _)), _) => mv.images.get(&(0x0009_0000 | *img as u32)).or_else(|| mv.images.get(&(*img as u32))).map(|im| (im.file_name.clone(), Vec2::new(*x1 as f32, *y1 as f32))),
                            _ => None,
                        };
                        let Some((file, off)) = tex else { continue };
                        let inv = matrix.inverse();
                        let (s0x, s0y) = inv.apply(b.xmin, b.ymin);
                        let (s1x, s1y) = inv.apply(b.xmax, b.ymax);
                        let src = [s0x.min(s1x) + off.x, s0y.min(s1y) + off.y, s0x.max(s1x) + off.x, s0y.max(s1y) + off.y];
                        let key = file.trim_end_matches(".tga").trim_end_matches(".dds").to_string();
                        if let Some((src, dst)) = clip_quad(src, corners, masks.as_slice()) {
                            out.push(Draw::Quad { tex: TexKey::Movie(key), src, dst, rgba: rgba([1.0; 4]) });
                        }
                        break;
                    }
                    // Drawn from the raster above.
                    Fill::Solid(_) | Fill::Gradient(_) => {}
                }
            }
        }
        // External image.
        if let Some(img) = &n.image {
            let p = |x: f32, y: f32| {
                let (px, py) = m.apply(x * 20.0, y * 20.0);
                Vec2::new(px, py) / 20.0
            };
            let dst = [p(0.0, 0.0), p(img.size.x, 0.0), p(img.size.x, img.size.y), p(0.0, img.size.y)];
            if let Some((src, dst)) = clip_quad([0.0, 0.0, img.size.x, img.size.y], dst, masks.as_slice()) {
                out.push(Draw::Quad { tex: TexKey::External(img.path.clone()), src, dst, rgba: rgba([1.0; 4]) });
            }
        }
        // Text.
        if n.text.is_some() {
            if let Some(l) = self.text_layout(id) {
                let text = l.lines.join("\n");
                if !text.trim().is_empty() {
                    let base = [l.color[0] as f32 / 255.0, l.color[1] as f32 / 255.0, l.color[2] as f32 / 255.0, l.color[3] as f32 / 255.0];
                    let rc = rgba(base);
                    let (xs, _, rot) = decompose(&m);
                    let (ox, oy) = m.apply((l.rect.min.x + 2.0 + l.margins[0]) * 20.0, (l.rect.min.y + 2.0) * 20.0);
                    let inner_w = (l.rect.width() - 4.0 - l.margins[0] - l.margins[1]).max(1.0);
                    if rc[3] > 0.002 {
                        out.push(Draw::Text { text, kind: l.kind, size: l.size * xs, origin: Vec2::new(ox, oy) / 20.0, width: inner_w * xs, line_h: l.line_h * xs, align: l.align, rot: rot.to_radians(), rgba: rc, spacing: l.letter_spacing * xs });
                    }
                }
            }
        }
        // Children in depth order; timeline masks (clipDepth) clip the depths above them.
        let mut clip_until: Vec<(i32, [Vec2; 4])> = Vec::new();
        for (d, cid) in &n.children {
            while clip_until.last().is_some_and(|(e, _)| *d > *e) {
                clip_until.pop();
                masks.pop();
            }
            let Some(cn) = self.node(*cid) else { continue };
            if let Some(cd) = cn.clip_depth {
                let cm = m.mul(&cn.matrix);
                if let Some(b) = self.local_bounds(*cid) {
                    let p = |x: f32, y: f32| {
                        let (px, py) = cm.apply(x * 20.0, y * 20.0);
                        Vec2::new(px, py) / 20.0
                    };
                    let q = [p(b.min.x, b.min.y), p(b.max.x, b.min.y), p(b.max.x, b.max.y), p(b.min.x, b.max.y)];
                    clip_until.push((cd, q));
                    masks.push(q);
                }
                continue;
            }
            self.draw_node(*cid, m, c, masks, out);
        }
        for _ in clip_until {
            masks.pop();
        }
        if pushed_mask {
            masks.pop();
        }
    }
}

/// A text field's layout (field space, pixels).
#[derive(Clone, Debug)]
pub struct TextLayoutInfo {
    pub lines: Vec<String>,
    /// Box (after `autoSize`).
    pub rect: Rect,
    pub size: f32,
    pub kind: FontKind,
    pub align: Justify,
    pub color: [u8; 4],
    pub text_w: f32,
    pub text_h: f32,
    pub line_h: f32,
    pub margins: [f32; 2],
    pub letter_spacing: f32,
}

fn transform_rect(m: &Matrix, r: Rect) -> Rect {
    let pts = [(r.min.x, r.min.y), (r.max.x, r.min.y), (r.max.x, r.max.y), (r.min.x, r.max.y)].map(|(x, y)| {
        let (px, py) = m.apply(x * 20.0, y * 20.0);
        Vec2::new(px, py) / 20.0
    });
    let mut out = Rect::from_corners(pts[0], pts[0]);
    for p in &pts[1..] {
        out = out.union_point(*p);
    }
    out
}

/// Crop a quad (texture `src` rectangle drawn on the parallelogram `dst`: TL, TR, BR, BL) by the
/// masks: each mask parallelogram is expressed in the quad's (u, v) space and the quad keeps
/// the part inside its bounding box there (exact for masks aligned with the quad, the HUD's
/// case). None when nothing is left.
fn clip_quad(src: [f32; 4], dst: [Vec2; 4], masks: &[[Vec2; 4]]) -> Option<([f32; 4], [Vec2; 4])> {
    if masks.is_empty() {
        return Some((src, dst));
    }
    let o = dst[0];
    let u = dst[1] - dst[0];
    let v = dst[3] - dst[0];
    let det = u.x * v.y - u.y * v.x;
    if det.abs() < 1e-6 {
        return None;
    }
    let to_uv = |p: Vec2| {
        let d = p - o;
        Vec2::new((d.x * v.y - d.y * v.x) / det, (u.x * d.y - u.y * d.x) / det)
    };
    let (mut u0, mut v0, mut u1, mut v1) = (0.0f32, 0.0f32, 1.0f32, 1.0f32);
    for m in masks {
        let pts = m.map(to_uv);
        let (mu0, mu1) = (pts.iter().map(|p| p.x).fold(f32::MAX, f32::min), pts.iter().map(|p| p.x).fold(f32::MIN, f32::max));
        let (mv0, mv1) = (pts.iter().map(|p| p.y).fold(f32::MAX, f32::min), pts.iter().map(|p| p.y).fold(f32::MIN, f32::max));
        u0 = u0.max(mu0);
        u1 = u1.min(mu1);
        v0 = v0.max(mv0);
        v1 = v1.min(mv1);
    }
    if u1 - u0 <= 1e-4 || v1 - v0 <= 1e-4 {
        return None;
    }
    let p = |a: f32, b: f32| o + u * a + v * b;
    let (sx0, sy0, sx1, sy1) = (src[0], src[1], src[2], src[3]);
    let nsrc = [sx0 + (sx1 - sx0) * u0, sy0 + (sy1 - sy0) * v0, sx0 + (sx1 - sx0) * u1, sy0 + (sy1 - sy0) * v1];
    Some((nsrc, [p(u0, v0), p(u1, v0), p(u1, v1), p(u0, v1)]))
}

/// Texture of a quad: one of the movie's own images (file name without extension) or an
/// external `Package.Texture`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TexKey {
    Movie(String),
    External(String),
    /// A vector shape's raster (`swf_shape::rasterize`): (movie key, shape id).
    Shape(u64, u16),
}

/// A flattened draw item (stage pixels).
#[derive(Clone, Debug)]
pub enum Draw {
    Quad { tex: TexKey, src: [f32; 4], dst: [Vec2; 4], rgba: [f32; 4] },
    Solid { dst: [Vec2; 4], rgba: [f32; 4] },
    /// `origin`: top-left of the first line's box; `width`: the field's inner width.
    Text { text: String, kind: FontKind, size: f32, origin: Vec2, width: f32, line_h: f32, align: Justify, rot: f32, rgba: [f32; 4], spacing: f32 },
}

// ---------------------------------------------------------------------------------------------
// Bevy: textures, views
// ---------------------------------------------------------------------------------------------

/// Textures of the stages: the movies' images and external `img://` textures, loaded on use.
#[derive(Resource, Default)]
pub struct StageTextures {
    pub movie: HashMap<String, Handle<Image>>,
    pub external: HashMap<String, Option<(Handle<Image>, Vec2)>>,
    /// The movies' vector shapes rasterized: (movie key, shape id).
    pub shapes: HashMap<(u64, u16), Handle<Image>>,
}

impl StageTextures {
    /// Decode every image the movie names from its package.
    pub fn load_movie(&mut self, content: &dishonored_assets::Content, package: &str, movie: &Movie, images: &mut Assets<Image>) {
        // Vector shapes (solid and gradient fills on their edges) as the player draws them.
        for (id, sh) in &movie.shapes {
            if self.shapes.contains_key(&(movie.key, *id)) {
                continue;
            }
            if let Some(r) = dishonored_assets::swf_shape::rasterize(sh) {
                let img = Image::new(
                    bevy::render::render_resource::Extent3d { width: r.width, height: r.height, depth_or_array_layers: 1 },
                    bevy::render::render_resource::TextureDimension::D2,
                    r.rgba,
                    bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                    bevy::asset::RenderAssetUsages::RENDER_WORLD,
                );
                self.shapes.insert((movie.key, *id), images.add(img));
            }
        }
        let Ok(pkg) = content.package(package) else { return };
        for im in movie.images.values() {
            let key = im.file_name.trim_end_matches(".tga").trim_end_matches(".dds").to_string();
            if self.movie.contains_key(&key) {
                continue;
            }
            let ti = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "Texture2D" && pkg.obj_path(ObjRef::Export(i)).rsplit('.').next().is_some_and(|n| n.eq_ignore_ascii_case(&key)));
            if let Some(ti) = ti {
                if let Ok(t) = dishonored_assets::texture::read_texture(content, &pkg, ti, 2048) {
                    self.movie.insert(key, images.add(to_image(&t)));
                }
            }
        }
    }

    /// An external texture by "Package.Texture" (`img://` path), with its pixel size.
    pub fn external(&mut self, content: &dishonored_assets::Content, path: &str, images: &mut Assets<Image>) -> Option<(Handle<Image>, Vec2)> {
        // Keyed without the `img://` scheme: what `ImageRef::path` (and so the draw's
        // `TexKey::External`) holds.
        let path_clean = path.trim_start_matches("img://");
        if let Some(v) = self.external.get(path_clean) {
            return v.clone();
        }
        let (pkg_name, tex) = path_clean.split_once('.')?;
        // "Group.Texture" names a package file (UI_portraits_objectives) or a group cooked into
        // another package (UI_EquipmentIcons, UI_ItemIcons_Small: Startup).
        let want = path_clean.to_ascii_lowercase();
        let mut from = |pkg: &ue3_package::Package, by_path: bool| -> Option<(Handle<Image>, Vec2)> {
            let ti = (0..pkg.exports.len()).find(|&i| {
                pkg.export_class(i) == "Texture2D" && {
                    let p = pkg.obj_path(ObjRef::Export(i));
                    if by_path { p.to_ascii_lowercase().ends_with(&want) } else { p.rsplit('.').next().is_some_and(|n| n.eq_ignore_ascii_case(tex)) }
                }
            })?;
            let t = dishonored_assets::texture::read_texture(content, pkg, ti, 1024).ok()?;
            Some((images.add(to_image(&t)), Vec2::new(t.width as f32, t.height as f32)))
        };
        let mut loaded = if content.has_package(pkg_name) { content.package(pkg_name).ok().and_then(|p| from(&*p, false)) } else { None };
        if loaded.is_none() {
            for host in ["Startup", "UI_HUD_SF"] {
                if let Ok(p) = content.package(host) {
                    loaded = from(&*p, true);
                    if loaded.is_some() {
                        break;
                    }
                }
            }
        }
        if loaded.is_none() {
            warn!("GFx external image not found: {path}");
        }
        self.external.insert(path_clean.to_string(), loaded.clone());
        loaded
    }
}

/// Root of a stage's UI entities.
#[derive(Component)]
pub struct StageView {
    /// Which stage (index in the owner's list).
    pub stage: usize,
    pub slots: Vec<Slot>,
}

#[derive(Clone, Copy)]
pub struct Slot {
    pub image: Entity,
    pub text: Entity,
}

#[derive(Component)]
pub struct SlotImage;
#[derive(Component)]
pub struct SlotText;

/// Spawn a full-window root for a stage.
pub fn spawn_view(commands: &mut Commands, stage: usize, z: i32) -> Entity {
    commands
        .spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, GlobalZIndex(z), Pickable::IGNORE, StageView { stage, slots: Vec::new() }))
        .id()
}

fn spawn_slot(commands: &mut Commands, root: Entity) -> Slot {
    let image = commands
        .spawn((ImageNode { image_mode: NodeImageMode::Stretch, ..default() }, Node { position_type: PositionType::Absolute, ..default() }, UiTransform::default(), Visibility::Hidden, Pickable::IGNORE, SlotImage, ChildOf(root)))
        .id();
    let text = commands
        .spawn((
            Text::new(""),
            TextFont { font_size: FontSize::Px(16.0), ..default() },
            TextColor(Color::WHITE),
            TextLayout::new(Justify::Left, LineBreak::NoWrap),
            LineHeight::Px(16.0),
            LetterSpacing::Px(0.0),
            Node { position_type: PositionType::Absolute, ..default() },
            UiTransform::default(),
            Visibility::Hidden,
            Pickable::IGNORE,
            SlotText,
            ChildOf(root),
        ))
        .id();
    Slot { image, text }
}

/// Window geometry of the 1280x720 stage ("show all", centred).
pub fn stage_xform(window: Vec2) -> (f32, Vec2) {
    let s = (window.x / STAGE.x).min(window.y / STAGE.y);
    (s, (window - STAGE * s) * 0.5)
}

/// Write draw items into a view's slot pool (spawning slots as needed; the item order is the
/// drawing order). Entities are written only when their values change.
#[allow(clippy::too_many_arguments)]
pub fn sync_view(
    commands: &mut Commands,
    root: Entity,
    view: &mut StageView,
    draws: &[Draw],
    textures: &StageTextures,
    fonts: Option<&crate::gfx_hud::GameFonts>,
    window: Vec2,
    images: &mut Query<(&mut ImageNode, &mut Node, &mut UiTransform, &mut Visibility), (With<SlotImage>, Without<SlotText>)>,
    texts: &mut Query<(&mut Text, &mut TextFont, &mut TextColor, &mut TextLayout, &mut LineHeight, &mut LetterSpacing, &mut Node, &mut UiTransform, &mut Visibility), (With<SlotText>, Without<SlotImage>)>,
) {
    let (s, off) = stage_xform(window);
    while view.slots.len() < draws.len() {
        let slot = spawn_slot(commands, root);
        view.slots.push(slot);
    }
    for (i, slot) in view.slots.iter().enumerate() {
        let d = draws.get(i);
        // Image half.
        if let Ok((mut img, mut node, mut xf, mut vis)) = images.get_mut(slot.image) {
            let quad = match d {
                Some(Draw::Quad { tex, src, dst, rgba }) => {
                    let h = match tex {
                        TexKey::Movie(k) => textures.movie.get(k).cloned(),
                        TexKey::External(p) => textures.external.get(p).cloned().flatten().map(|(h, _)| h),
                        TexKey::Shape(m, id) => textures.shapes.get(&(*m, *id)).cloned(),
                    };
                    h.map(|h| (h, Some(Rect::new(src[0], src[1], src[2], src[3])), *dst, *rgba))
                }
                Some(Draw::Solid { dst, rgba }) => Some((Handle::<Image>::default(), None, *dst, *rgba)),
                _ => None,
            };
            match quad {
                Some((h, rect, dst, rgba)) => {
                    let p = |c: Vec2| c * s + off;
                    let (tl, tr, br, bl) = (p(dst[0]), p(dst[1]), p(dst[2]), p(dst[3]));
                    let e1 = tr - tl;
                    let e2 = bl - tl;
                    let mirrored = e1.x * e2.y - e1.y * e2.x < 0.0;
                    let (w, hgt) = (e1.length(), e2.length());
                    let c = (tl + br) * 0.5;
                    crate::gfx_hud::place(&mut node, px(c.x - w * 0.5), px(c.y - hgt * 0.5), Some(px(w)), Some(px(hgt)));
                    let dir = if mirrored { -e1 } else { e1 };
                    let r = Rot2::radians(dir.y.atan2(dir.x));
                    if xf.rotation != r {
                        xf.rotation = r;
                    }
                    let color = Color::srgba(rgba[0], rgba[1], rgba[2], rgba[3]);
                    if img.image != h {
                        img.image = h;
                    }
                    if img.rect != rect {
                        img.rect = rect;
                    }
                    if img.color != color {
                        img.color = color;
                    }
                    if img.flip_x != mirrored {
                        img.flip_x = mirrored;
                    }
                    vis.set_if_neq(Visibility::Inherited);
                }
                None => {
                    vis.set_if_neq(Visibility::Hidden);
                }
            }
        }
        // Text half.
        if let Ok((mut text, mut font, mut color, mut layout, mut lh, mut ls, mut node, mut xf, mut vis)) = texts.get_mut(slot.text) {
            match d {
                Some(Draw::Text { text: t, kind, size, origin, width, line_h, align, rot, rgba, spacing }) => {
                    crate::gfx_hud::set_text(&mut text, t.clone());
                    crate::gfx_hud::set_font_size(&mut font, (size * s).max(1.0));
                    if let Some(f) = fonts {
                        let want: bevy::text::FontSource = match kind {
                            FontKind::Normal => f.normal.clone().into(),
                            FontKind::Title => f.title.clone().into(),
                        };
                        if font.font != want {
                            font.font = want;
                        }
                    }
                    let c = Color::srgba(rgba[0], rgba[1], rgba[2], rgba[3]);
                    if color.0 != c {
                        color.0 = c;
                    }
                    if layout.justify != *align {
                        layout.justify = *align;
                    }
                    let want_lh = LineHeight::Px((line_h * s).max(1.0));
                    if *lh != want_lh {
                        *lh = want_lh;
                    }
                    let want_ls = LetterSpacing::Px(spacing * s);
                    if *ls != want_ls {
                        *ls = want_ls;
                    }
                    // The box rotates about its centre; place it so its top-left is `origin`.
                    let lines = t.lines().count().max(1) as f32;
                    let (w, hgt) = (width * s, line_h * s * lines);
                    let o = *origin * s + off;
                    let (sn, cs) = rot.sin_cos();
                    let half = Vec2::new(w * 0.5, hgt * 0.5);
                    let centre = o + Vec2::new(half.x * cs - half.y * sn, half.x * sn + half.y * cs);
                    crate::gfx_hud::place(&mut node, px(centre.x - half.x), px(centre.y - half.y), Some(px(w.max(1.0))), None);
                    let r = Rot2::radians(*rot);
                    if xf.rotation != r {
                        xf.rotation = r;
                    }
                    vis.set_if_neq(Visibility::Inherited);
                }
                _ => {
                    vis.set_if_neq(Visibility::Hidden);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_ends() {
        for e in [Ease::Linear, Ease::StrongIn, Ease::StrongOut, Ease::StrongInOut, Ease::BackIn, Ease::BackOut, Ease::BackInOut] {
            assert!(e.at(0.0).abs() < 1e-5, "{e:?}");
            assert!((e.at(1.0) - 1.0).abs() < 1e-5, "{e:?}");
        }
        // Strong.easeOut at half time: 1 - 0.5^5.
        assert!((Ease::StrongOut.at(0.5) - (1.0 - 0.5f32.powi(5))).abs() < 1e-6);
        // Back.easeOut overshoots.
        assert!(Ease::BackOut.at(0.8) > 1.0);
    }

    #[test]
    fn html_strip() {
        assert_eq!(strip_html("a<br/>b <font color='#fff'>c</font> &amp; d"), "a\nb c & d");
        assert_eq!(strip_html("<p>x</p>"), "x");
    }

    #[test]
    fn mask_crop() {
        let dst = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)];
        let mask = [Vec2::new(0.0, 5.0), Vec2::new(10.0, 5.0), Vec2::new(10.0, 20.0), Vec2::new(0.0, 20.0)];
        let (src, d) = clip_quad([0.0, 0.0, 100.0, 100.0], dst, &[mask]).unwrap();
        assert_eq!(src, [0.0, 50.0, 100.0, 100.0]);
        assert_eq!(d[0], Vec2::new(0.0, 5.0));
        assert!(clip_quad([0.0, 0.0, 1.0, 1.0], dst, &[[Vec2::new(20.0, 0.0), Vec2::new(30.0, 0.0), Vec2::new(30.0, 5.0), Vec2::new(20.0, 5.0)]]).is_none());
    }
}
