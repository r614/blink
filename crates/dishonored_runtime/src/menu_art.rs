//! Drawing Dishonored's Scaleform menu movies with Bevy UI.
//!
//! A menu screen is rebuilt into a [`DisplayList`] of primitives in movie stage coordinates
//! (1280x720): textured quads flattened from the movie's sprites (atlas sub-rectangle + stage
//! corners, at the frame a state label's timeline is showing), text in the game's fonts laid
//! out in the movie's own text-field rectangles, and hit rectangles for interaction.
//! [`spawn_list_cached`] turns the list into UI entities (updating them in place while only
//! geometry and colours change, as during a tween); [`layout_prims`] fits the stage to the
//! window every frame ("show all", centred), like `gfx_hud`.
//!
//! ActionScript is not executed. What the movies' classes do at runtime is reproduced by the
//! screen builders: they pick the state label each clip's timeline was sent to and how long ago
//! (the timeline is then played the way the Flash player plays it, honouring the frame scripts'
//! `stop()` / `gotoAndPlay()`, see [`FrameScript`]), and they override clip properties
//! (`_x`, `_y`, `_xscale`, `_yscale`, `_rotation`, `_alpha`, `_visible`) the way the classes'
//! tweens do ([`Props`], `menu_anim.rs`).
//!
//! Not drawn: the GFx 3D extension properties (`_z`, `_xrotation`, `_yrotation`); see
//! `research/dishonored/MENUS.md` ("3D properties").

use crate::map_render::to_image;
use bevy::prelude::*;
use dishonored_assets::swf::{ColorTransform, Fill, Matrix, Movie, Quad, Sprite, TextPlacement};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use ue3_package::ObjRef;
use ue3_package::properties::find;

pub const STAGE: Vec2 = Vec2::new(1280.0, 720.0);

/// What a timeline frame's `DoAction` does with the playhead (the menus' frame scripts are
/// one-liners: `stop()`, `play()`, `gotoAndPlay("loop")`).
#[derive(Clone, Debug, PartialEq)]
pub enum FrameScript {
    Stop,
    Play,
    Goto { label: Option<String>, frame: Option<usize>, play: bool },
}

/// Frame scripts per sprite id (0 = the root timeline), by frame index.
pub type Scripts = HashMap<u16, BTreeMap<usize, FrameScript>>;

/// A parsed movie plus its external textures and frame scripts, loaded once.
pub struct MovieArt {
    pub movie: Movie,
    pub textures: HashMap<String, Handle<Image>>,
    pub scripts: Scripts,
}

pub(crate) fn movie_raw(pkg: &ue3_package::Package, name: &str) -> Option<Vec<u8>> {
    let i = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SwfMovie" && pkg.obj_path(ObjRef::Export(i)).ends_with(name))?;
    let h = pkg.read_object(i).ok()?;
    let (n, raw) = find(&h.properties, "RawData").and_then(|p| p.array_raw())?;
    Some(raw[..n].to_vec())
}

fn texture_index(pkg: &ue3_package::Package, name: &str) -> Option<usize> {
    let want = [name.to_string(), name.trim_end_matches(" -nopack").to_string()];
    (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "Texture2D" && pkg.obj_path(ObjRef::Export(i)).rsplit('.').next().is_some_and(|n| want.iter().any(|w| n.eq_ignore_ascii_case(w))))
}

/// Walk the movie's tags and keep each sprite's frame scripts.
pub fn parse_frame_scripts(raw: &[u8]) -> Scripts {
    let mut out = Scripts::new();
    if raw.len() < 9 {
        return out;
    }
    let nbits = (raw[8] >> 3) as usize;
    let pos = 8 + (5 + 4 * nbits).div_ceil(8) + 4;
    script_tags(raw, pos, raw.len(), 0, &mut out);
    out
}

fn script_tags(raw: &[u8], mut pos: usize, end: usize, sprite: u16, out: &mut Scripts) {
    let mut frame = 0usize;
    while pos + 2 <= end {
        let h = u16::from_le_bytes([raw[pos], raw[pos + 1]]);
        pos += 2;
        let code = h >> 6;
        let mut len = (h & 0x3f) as usize;
        if len == 0x3f {
            if pos + 4 > end {
                break;
            }
            len = u32::from_le_bytes([raw[pos], raw[pos + 1], raw[pos + 2], raw[pos + 3]]) as usize;
            pos += 4;
        }
        let body = pos;
        let tend = (pos + len).min(end);
        pos = tend;
        match code {
            0 => break,
            1 => frame += 1,
            12 => {
                if let Some(s) = action_script(&raw[body..tend]) {
                    out.entry(sprite).or_default().insert(frame, s);
                }
            }
            39 if sprite == 0 && tend >= body + 4 => {
                let id = u16::from_le_bytes([raw[body], raw[body + 1]]);
                script_tags(raw, body + 4, tend, id, out);
            }
            _ => {}
        }
    }
}

/// The playhead effect of one `DoAction`: its first `stop()` / `play()`, after an optional
/// `gotoLabel` / `gotoFrame` (a goto without `play()` leaves the timeline stopped there).
fn action_script(b: &[u8]) -> Option<FrameScript> {
    let mut i = 0;
    let mut goto: Option<(Option<String>, Option<usize>)> = None;
    while i < b.len() {
        let op = b[i];
        i += 1;
        if op == 0 {
            break;
        }
        let (data, next) = if op >= 0x80 {
            if i + 2 > b.len() {
                break;
            }
            let l = u16::from_le_bytes([b[i], b[i + 1]]) as usize;
            (&b[(i + 2).min(b.len())..(i + 2 + l).min(b.len())], i + 2 + l)
        } else {
            (&b[0..0], i)
        };
        match op {
            0x07 => {
                return Some(match goto {
                    Some((label, frame)) => FrameScript::Goto { label, frame, play: false },
                    None => FrameScript::Stop,
                });
            }
            0x06 => {
                return Some(match goto {
                    Some((label, frame)) => FrameScript::Goto { label, frame, play: true },
                    None => FrameScript::Play,
                });
            }
            0x8C => {
                let n = data.iter().position(|c| *c == 0).unwrap_or(data.len());
                goto = Some((Some(String::from_utf8_lossy(&data[..n]).into_owned()), None));
            }
            0x81 if data.len() >= 2 => goto = Some((None, Some(u16::from_le_bytes([data[0], data[1]]) as usize))),
            _ => {}
        }
        i = next;
    }
    goto.map(|(label, frame)| FrameScript::Goto { label, frame, play: false })
}

impl MovieArt {
    /// Parse `movie` from `package` and decode every external image it names.
    pub fn load(content: &dishonored_assets::Content, package: &str, movie: &str, images: &mut Assets<Image>) -> Option<Self> {
        let pkg: Arc<ue3_package::Package> = content.package(package).ok()?;
        let raw = movie_raw(&pkg, movie)?;
        let scripts = parse_frame_scripts(&raw);
        let movie = Movie::parse(&raw).ok()?;
        let mut textures = HashMap::new();
        for im in movie.images.values() {
            let key = im.file_name.trim_end_matches(".tga").trim_end_matches(".dds").to_string();
            if textures.contains_key(&key) {
                continue;
            }
            let Some(ti) = texture_index(&pkg, &key) else { continue };
            if let Ok(t) = dishonored_assets::texture::read_texture(content, &pkg, ti, 2048) {
                textures.insert(key, images.add(to_image(&t)));
            }
        }
        Some(Self { movie, textures, scripts })
    }

    pub fn export(&self, name: &str) -> Option<u16> {
        self.movie.exports.get(name).copied()
    }

    /// Frames per second of the movie (the menus run at 30).
    pub fn fps(&self) -> f32 {
        if self.movie.frame_rate > 0.0 { self.movie.frame_rate } else { 30.0 }
    }

    /// Character placed under `name` in the root timeline (first frame where it appears).
    pub fn root_child(&self, name: &str) -> Option<(u16, Matrix)> {
        for f in 0..self.movie.root.frames.len() {
            if let Some(p) = self.movie.display_list(&self.movie.root, f).into_iter().find(|p| p.name.as_deref() == Some(name)) {
                return Some((p.character?, p.matrix.unwrap_or_default()));
            }
        }
        None
    }

    /// Child placement `name` inside sprite `parent` at frame 0.
    pub fn child(&self, parent: u16, name: &str) -> Option<(u16, Matrix)> {
        let s = self.movie.sprites.get(&parent)?;
        let p = self.movie.display_list(s, 0).into_iter().find(|p| p.name.as_deref() == Some(name))?;
        Some((p.character?, p.matrix.unwrap_or_default()))
    }

    /// Authored alpha (percent) of child `name` of `parent` at frame 0.
    pub fn child_alpha(&self, parent: u16, name: &str) -> f32 {
        self.movie
            .sprites
            .get(&parent)
            .and_then(|s| self.movie.display_list(s, 0).into_iter().find(|p| p.name.as_deref() == Some(name)))
            .and_then(|p| p.color)
            .map(|c| c.mul[3] * 100.0)
            .unwrap_or(100.0)
    }

    /// Named children of `parent` at frame 0, in depth order: (name, character, matrix).
    pub fn children(&self, parent: u16) -> Vec<(String, u16, Matrix)> {
        let Some(s) = self.movie.sprites.get(&parent) else { return Vec::new() };
        self.movie.display_list(s, 0).into_iter().filter_map(|p| Some((p.name?, p.character?, p.matrix.unwrap_or_default()))).collect()
    }

    pub fn sprite(&self, id: u16) -> Option<&Sprite> {
        self.movie.sprites.get(&id)
    }

    pub fn script(&self, sprite: u16, frame: usize) -> Option<&FrameScript> {
        self.scripts.get(&sprite).and_then(|m| m.get(&frame))
    }

    /// Run the frame script of `f` (and of goto targets): returns the frame the playhead is
    /// on and whether it keeps playing.
    fn enter(&self, id: u16, s: &Sprite, mut f: usize, mut playing: bool) -> (usize, bool) {
        for _ in 0..4 {
            match self.script(id, f) {
                Some(FrameScript::Stop) => return (f, false),
                Some(FrameScript::Play) => return (f, true),
                Some(FrameScript::Goto { label, frame, play }) => {
                    let t = label.as_deref().and_then(|l| Movie::label_frame(s, l)).or(*frame).unwrap_or(f).min(s.frames.len().saturating_sub(1));
                    playing = *play;
                    if t == f {
                        return (f, playing);
                    }
                    f = t;
                }
                None => return (f, playing),
            }
        }
        (f, playing)
    }

    /// The frame shown `ticks` frames after `gotoAndPlay(start)` on sprite `id`, and whether
    /// the timeline is still playing then. Past the last frame a playing timeline wraps to the
    /// first, as in Flash.
    pub fn play_frame(&self, id: u16, s: &Sprite, start: usize, ticks: u32) -> (usize, bool) {
        let n = s.frames.len().max(1);
        let (mut f, mut playing) = self.enter(id, s, start.min(n - 1), true);
        let step = |f: usize, playing: bool| -> (usize, bool) { if playing { self.enter(id, s, (f + 1) % n, true) } else { (f, false) } };
        let ticks = ticks as usize;
        let warm = ticks.min(n);
        for _ in 0..warm {
            if !playing {
                return (f, false);
            }
            (f, playing) = step(f, playing);
        }
        if ticks <= n || !playing {
            return (f, playing);
        }
        // Looping: find the period from here and skip whole periods.
        let (f0, mut p) = (f, 0usize);
        let (mut g, mut pl) = (f, playing);
        loop {
            (g, pl) = step(g, pl);
            p += 1;
            if !pl || g == f0 || p > n {
                break;
            }
        }
        if !pl {
            // Stops within the next round.
            let rest = ticks - n;
            if rest >= p {
                return (g, false);
            }
            for _ in 0..rest {
                (f, playing) = step(f, playing);
            }
            return (f, playing);
        }
        let rest = (ticks - n) % p.max(1);
        for _ in 0..rest {
            (f, playing) = step(f, playing);
        }
        (f, playing)
    }

    /// Settled frame of a state label: where `gotoAndPlay(label)` stops. A looping state shows
    /// its label frame.
    pub fn settled_frame(&self, id: u16, s: &Sprite, label: &str) -> usize {
        let Some(start) = Movie::label_frame(s, label) else { return 0 };
        let (f, playing) = self.play_frame(id, s, start, s.frames.len() as u32);
        if playing { start } else { f }
    }

    /// Stage bounds (pixels) of a character flattened at `m`.
    pub fn bounds(&self, character: u16, m: Matrix, states: &States) -> Option<Rect> {
        self.bounds_fx(character, m, states, &|_| None)
    }

    /// [`MovieArt::bounds`] with property overrides.
    pub fn bounds_fx(&self, character: u16, m: Matrix, states: &States, ovr: Ovr) -> Option<Rect> {
        let mut out = Vec::new();
        self.walk(character, m, ColorTransform::default(), "", states, ovr, &mut out);
        let mut r: Option<Rect> = None;
        for f in &out {
            let pts: Vec<(f32, f32)> = match f {
                Flat::Quad(q) => q.dst.to_vec(),
                Flat::Solid(d, _, _) => d.to_vec(),
                Flat::Text(_) => continue,
            };
            for c in pts {
                let p = Vec2::new(c.0, c.1);
                r = Some(r.map_or(Rect::from_corners(p, p), |r| r.union_point(p)));
            }
        }
        r
    }

    /// Flatten a character: bitmap quads, solid fills and text fields in drawing order.
    /// `states` picks each sprite's frame, `ovr` overrides named clips' properties by path
    /// (paths are relative to `character`, e.g. `_menu_mc/_lineUp_mc`).
    #[allow(clippy::too_many_arguments)]
    pub fn walk(&self, character: u16, m: Matrix, color: ColorTransform, path: &str, states: &States, ovr: Ovr, out: &mut Vec<Flat>) {
        self.walk_masked(character, m, color, path, states, ovr, &mut Vec::new(), out);
    }

    /// [`MovieArt::walk`] inside the timeline masks in effect (stage parallelograms): a
    /// placement with a `clipDepth` is a mask layer, never drawn; the depths above it up to its
    /// `clipDepth` are cropped to its extent (the mask's bounds in its own space, placed by its
    /// matrix: exact for the rectangular masks the menus use, as `gfx_stage` does). Text fields
    /// are not cropped.
    #[allow(clippy::too_many_arguments)]
    fn walk_masked(&self, character: u16, m: Matrix, color: ColorTransform, path: &str, states: &States, ovr: Ovr, masks: &mut Vec<[Vec2; 4]>, out: &mut Vec<Flat>) {
        let mv = &self.movie;
        if let Some(t) = mv.edit_texts.get(&character) {
            out.push(Flat::Text(TextPlacement { id: character, text: t.clone(), matrix: m, color, path: path.to_string() }));
            return;
        }
        if let Some(shape) = mv.shapes.get(&character) {
            let b = shape.bounds;
            let corners = [(b.xmin, b.ymin), (b.xmax, b.ymin), (b.xmax, b.ymax), (b.xmin, b.ymax)];
            let dst = corners.map(|(x, y)| {
                let (px, py) = m.apply(x, y);
                (px / 20.0, py / 20.0)
            });
            for fill in &shape.fills {
                match fill {
                    Fill::Bitmap { bitmap, matrix, .. } => {
                        let (tex, off) = match (mv.sub_images.get(bitmap), mv.images.get(&(*bitmap as u32))) {
                            (None, Some(im)) => (im.file_name.clone(), (0.0, 0.0)),
                            (Some((img, x1, y1, _, _)), _) => match mv.images.get(&(0x0009_0000 | *img as u32)).or_else(|| mv.images.get(&(*img as u32))) {
                                Some(im) => (im.file_name.clone(), (*x1 as f32, *y1 as f32)),
                                None => continue,
                            },
                            _ => continue,
                        };
                        let inv = matrix.inverse();
                        let (s0x, s0y) = inv.apply(b.xmin, b.ymin);
                        let (s1x, s1y) = inv.apply(b.xmax, b.ymax);
                        let texture = tex.trim_end_matches(".tga").trim_end_matches(".dds").to_string();
                        let src = [s0x.min(s1x) + off.0, s0y.min(s1y) + off.1, s0x.max(s1x) + off.0, s0y.max(s1y) + off.1];
                        if let Some((src, dst)) = crop(src, dst, masks) {
                            out.push(Flat::Quad(Quad { texture, src, dst, color, path: path.to_string(), character }));
                        }
                        break;
                    }
                    Fill::Solid(c) => {
                        let rgba = std::array::from_fn(|i| (c[i] as f32 / 255.0 * color.mul[i] + color.add[i]).clamp(0.0, 1.0));
                        if let Some((_, dst)) = crop([0.0, 0.0, 1.0, 1.0], dst, masks) {
                            out.push(Flat::Solid(dst, rgba, path.to_string()));
                        }
                        break;
                    }
                    Fill::Gradient(_) => {}
                }
            }
            return;
        }
        let Some(sprite) = mv.sprites.get(&character) else { return };
        let frame = states.frame(path, character, sprite, self);
        // Mask layers in effect in this timeline: (last depth they clip, its extent).
        let mut clip_until: Vec<u16> = Vec::new();
        for p in mv.display_list(sprite, frame) {
            while clip_until.last().is_some_and(|e| p.depth > *e) {
                clip_until.pop();
                masks.pop();
            }
            let Some(ch) = p.character else { continue };
            let child = match &p.name {
                Some(n) if path.is_empty() => n.clone(),
                Some(n) => format!("{path}/{n}"),
                None => path.to_string(),
            };
            let authored = p.matrix.unwrap_or_default();
            let mut local_color = p.color.unwrap_or_default();
            let local = match p.name.as_ref().and_then(|_| ovr(&child)) {
                Some(o) => {
                    if !o.visible {
                        continue;
                    }
                    local_color.mul[3] = o.alpha / 100.0;
                    o.matrix_like(&authored)
                }
                None => authored,
            };
            let pm = m.mul(&local);
            let pc = ColorTransform { mul: std::array::from_fn(|i| color.mul[i] * local_color.mul[i]), add: std::array::from_fn(|i| color.add[i] + local_color.add[i] * color.mul[i]) };
            if let Some(cd) = p.clip_depth {
                // The mask's extent: its own bounds (flattened in its space) placed by its matrix.
                let mut inner = Vec::new();
                self.walk_masked(ch, Matrix::default(), ColorTransform::default(), &child, states, ovr, &mut Vec::new(), &mut inner);
                let mut r: Option<Rect> = None;
                for f in &inner {
                    let pts: &[(f32, f32)] = match f {
                        Flat::Quad(q) => &q.dst,
                        Flat::Solid(d, _, _) => d,
                        Flat::Text(_) => continue,
                    };
                    for c in pts {
                        let v = Vec2::new(c.0, c.1);
                        r = Some(r.map_or(Rect::from_corners(v, v), |r| r.union_point(v)));
                    }
                }
                let q = r.map(|r| {
                    [(r.min.x, r.min.y), (r.max.x, r.min.y), (r.max.x, r.max.y), (r.min.x, r.max.y)].map(|(x, y)| {
                        let (px, py) = pm.apply(x * 20.0, y * 20.0);
                        Vec2::new(px, py) / 20.0
                    })
                });
                // An empty mask shows nothing below it.
                masks.push(q.unwrap_or([Vec2::ZERO; 4]));
                clip_until.push(cd);
                continue;
            }
            self.walk_masked(ch, pm, pc, &child, states, ovr, masks, out);
        }
        for _ in clip_until {
            masks.pop();
        }
    }
}

/// Crop a flattened quad to the masks in effect (`gfx_stage::clip_quad`): None when nothing
/// of it is left.
fn crop(src: [f32; 4], dst: [(f32, f32); 4], masks: &[[Vec2; 4]]) -> Option<([f32; 4], [(f32, f32); 4])> {
    if masks.is_empty() {
        return Some((src, dst));
    }
    let (src, d) = crate::gfx_stage::clip_quad(src, dst.map(|(x, y)| Vec2::new(x, y)), masks)?;
    Some((src, d.map(|p| (p.x, p.y))))
}

/// One flattened element of a movie, in drawing order.
#[derive(Clone)]
pub enum Flat {
    Quad(Quad),
    /// Solid fill: stage corners, RGBA, path.
    Solid([(f32, f32); 4], [f32; 4], String),
    Text(TextPlacement),
}

/// Property overrides by clip path (see [`MovieArt::walk`]).
pub type Ovr<'a> = &'a dyn Fn(&str) -> Option<Props>;

/// The AS2 `MovieClip` properties the menus' scripts set and tween.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Props {
    /// Stage pixels in the parent's space.
    pub x: f32,
    pub y: f32,
    /// Percent.
    pub xscale: f32,
    pub yscale: f32,
    /// Degrees.
    pub rotation: f32,
    /// Percent.
    pub alpha: f32,
    pub visible: bool,
}

impl Default for Props {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, xscale: 100.0, yscale: 100.0, rotation: 0.0, alpha: 100.0, visible: true }
    }
}

impl Props {
    /// The properties a placement matrix (and alpha, percent) reads back as.
    pub fn from_matrix(m: &Matrix, alpha: f32) -> Self {
        let xs = (m.a * m.a + m.b * m.b).sqrt();
        let det = m.a * m.d - m.b * m.c;
        let ys = (m.c * m.c + m.d * m.d).sqrt() * if det < 0.0 { -1.0 } else { 1.0 };
        Self { x: m.tx / 20.0, y: m.ty / 20.0, xscale: xs * 100.0, yscale: ys * 100.0, rotation: m.b.atan2(m.a).to_degrees(), alpha, visible: true }
    }

    /// Matrix of these properties (rotation then scale, no skew).
    pub fn matrix(&self) -> Matrix {
        let (s, c) = self.rotation.to_radians().sin_cos();
        let (sx, sy) = (self.xscale / 100.0, self.yscale / 100.0);
        Matrix { a: sx * c, b: sx * s, c: -sy * s, d: sy * c, tx: self.x * 20.0, ty: self.y * 20.0 }
    }

    /// Matrix of these properties keeping the authored linear part (and its skew) when scale
    /// and rotation are unchanged from it.
    pub fn matrix_like(&self, authored: &Matrix) -> Matrix {
        let a = Props::from_matrix(authored, self.alpha);
        if (a.xscale - self.xscale).abs() < 1e-3 && (a.yscale - self.yscale).abs() < 1e-3 && (a.rotation - self.rotation).abs() < 1e-3 {
            Matrix { tx: self.x * 20.0, ty: self.y * 20.0, ..*authored }
        } else {
            self.matrix()
        }
    }
}

/// A standalone Texture2D from a package, with its size in pixels.
pub fn load_texture(content: &dishonored_assets::Content, package: &str, name: &str, images: &mut Assets<Image>) -> Option<(Handle<Image>, Vec2)> {
    let pkg = content.package(package).ok()?;
    let ti = texture_index(&pkg, name)?;
    let t = dishonored_assets::texture::read_texture(content, &pkg, ti, 2048).ok()?;
    let size = Vec2::new(t.width as f32, t.height as f32);
    Some((images.add(to_image(&t)), size))
}

/// Which state label each sprite shows, per placement path (exact path, or `*` for every
/// sprite that has the label), optionally with the seconds since the clip was sent to it
/// (`gotoAndPlay`): the timeline is then played that far. Without a time the label's settled
/// frame (where its frame scripts stop) is shown.
///
/// With a `clock` (seconds since the clips were created), sprites no entry applies to play
/// their timeline from frame 0 as Flash does (a sprite whose first frame does not `stop()`
/// animates on its own: the Corvo mask, glows, compass loops).
#[derive(Default, Clone)]
pub struct States {
    pub list: Vec<(String, String, Option<f32>)>,
    pub clock: Option<f32>,
}

impl States {
    pub fn all(label: &str) -> Self {
        Self { list: vec![("*".into(), label.into(), None)], clock: None }
    }
    /// Every sprite with `label`, `secs` after it was sent there.
    pub fn all_timed(label: &str, secs: f32) -> Self {
        Self { list: vec![("*".into(), label.into(), Some(secs))], clock: None }
    }
    pub fn with(mut self, path: &str, label: &str) -> Self {
        self.list.push((path.into(), label.into(), None));
        self
    }
    pub fn with_timed(mut self, path: &str, label: &str, secs: f32) -> Self {
        self.list.push((path.into(), label.into(), Some(secs)));
        self
    }
    /// Free-running timelines `secs` after creation (see the type's docs).
    pub fn clock(mut self, secs: f32) -> Self {
        self.clock = Some(secs);
        self
    }
    pub fn frame(&self, path: &str, id: u16, s: &Sprite, art: &MovieArt) -> usize {
        let f = self.labelled_frame(path, id, s, art);
        match (f, self.clock) {
            (Some(f), _) => f,
            (None, Some(t)) if s.frames.len() > 1 => art.play_frame(id, s, 0, (t.max(0.0) * art.fps()) as u32).0,
            _ => 0,
        }
    }

    fn labelled_frame(&self, path: &str, id: u16, s: &Sprite, art: &MovieArt) -> Option<usize> {
        for (p, l, secs) in self.list.iter().rev() {
            // "#N": a fixed frame (sprites whose frame the script picks, e.g. random strip variants).
            if let Some(n) = l.strip_prefix('#').and_then(|n| n.parse::<usize>().ok()) {
                if p == path || p == "*" {
                    return Some(n.min(s.frames.len().saturating_sub(1)));
                }
                continue;
            }
            // "@N" with a time: `gotoAndPlay(N)` (a frame number instead of a label).
            if let Some(n) = l.strip_prefix('@').and_then(|n| n.parse::<usize>().ok()) {
                if p == path {
                    let t = secs.unwrap_or(0.0);
                    return Some(art.play_frame(id, s, n, (t.max(0.0) * art.fps()) as u32).0);
                }
                continue;
            }
            if p == "*" || p == path {
                if let Some(start) = Movie::label_frame(s, l) {
                    return Some(match secs {
                        Some(t) => art.play_frame(id, s, start, (t.max(0.0) * art.fps()) as u32).0,
                        None => art.settled_frame(id, s, l),
                    });
                }
            }
        }
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum FontKind {
    /// `$NormalFont` ("Emerge BF").
    Normal,
    /// `$TitleFont` ("ChaletComprime-CologneEighty").
    Title,
}

/// Advance widths of the two menu fonts (`DisFonts_SF` `fonts_efigs`), for the layouts the
/// movies compute from `textWidth` (button widths, autoSize fields).
#[derive(Default, Clone)]
pub struct FontMetrics {
    normal: HashMap<char, f32>,
    title: HashMap<char, f32>,
}

impl FontMetrics {
    pub fn load() -> Self {
        let mut m = FontMetrics::default();
        let Ok(content) = dishonored_assets::Content::locate() else { return m };
        let Some(movie) = content.package("DisFonts_SF").ok().and_then(|p| movie_raw(&p, "fonts_efigs")).and_then(|r| Movie::parse(&r).ok()) else { return m };
        for f in &movie.fonts {
            let table = if f.name.starts_with("Emerge") {
                &mut m.normal
            } else if f.name.starts_with("Chalet") {
                &mut m.title
            } else {
                continue;
            };
            for (i, code) in f.codes.iter().enumerate() {
                if let (Some(c), Some(a)) = (char::from_u32(*code as u32), f.advances.get(i)) {
                    table.insert(c, *a as f32 / 1024.0);
                }
            }
        }
        m
    }

    /// Width in pixels of `text` at `size` px (no kerning; unknown glyphs count half an em).
    pub fn width(&self, kind: FontKind, text: &str, size: f32) -> f32 {
        let t = match kind {
            FontKind::Normal => &self.normal,
            FontKind::Title => &self.title,
        };
        if t.is_empty() {
            return text.chars().count() as f32 * size * 0.5;
        }
        text.chars().map(|c| t.get(&c).copied().unwrap_or(0.5)).sum::<f32>() * size
    }

    /// `TextField.textWidth + 4` (the autoSize field width: 2 px gutter each side).
    pub fn field_width(&self, kind: FontKind, text: &str, size: f32) -> f32 {
        self.width(kind, text, size) + 4.0
    }
}

#[derive(Clone)]
pub enum Prim {
    Quad { tex: Handle<Image>, src: [f32; 4], dst: [(f32, f32); 4], color: [f32; 4] },
    /// `top`: wrap inside `rect` from its top edge (multiline fields) instead of centring one line.
    Text { text: String, rect: Rect, size: f32, font: FontKind, color: Color, align: Justify, top: bool, rot: f32 },
    Fill { rect: Rect, color: Color },
}

/// What an interactive rectangle does; interpreted by the screen that built it.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub rect: Rect,
    /// Keyboard focus order group (items of one list share a group).
    pub nav: bool,
}

#[derive(Default, Clone)]
pub struct DisplayList {
    pub prims: Vec<Prim>,
    pub items: Vec<Item>,
}

/// Text colour of the menus (`#e3f2d6`) and the dimmed variant (`#828a7b`).
pub const TEXT: Color = Color::srgb(227.0 / 255.0, 242.0 / 255.0, 214.0 / 255.0);
pub const TEXT_DIM: Color = Color::srgb(130.0 / 255.0, 138.0 / 255.0, 123.0 / 255.0);

pub fn px_matrix(x: f32, y: f32) -> Matrix {
    Matrix { tx: x * 20.0, ty: y * 20.0, ..Default::default() }
}

fn is_mask(path: &str) -> bool {
    path.to_ascii_lowercase().contains("mask")
}

impl DisplayList {
    /// Flatten `character` at stage matrix `m` (twips) into quads.
    pub fn movie(&mut self, art: &MovieArt, character: u16, m: Matrix, states: &States, alpha: f32) {
        self.movie_fx(art, character, m, states, alpha, &|_| None, false);
    }

    /// [`DisplayList::movie`] with clip property overrides; `solids` also draws the solid fills
    /// (backdrops such as the main menu band) in order with the bitmaps.
    #[allow(clippy::too_many_arguments)]
    pub fn movie_fx(&mut self, art: &MovieArt, character: u16, m: Matrix, states: &States, alpha: f32, ovr: Ovr, solids: bool) {
        self.movie_fx_solids(art, character, m, states, alpha, ovr, &|_| solids);
    }

    /// [`DisplayList::movie_fx`] drawing the solid fills whose path `solid` accepts.
    #[allow(clippy::too_many_arguments)]
    pub fn movie_fx_solids(&mut self, art: &MovieArt, character: u16, m: Matrix, states: &States, alpha: f32, ovr: Ovr, solid: &dyn Fn(&str) -> bool) {
        let mut out = Vec::new();
        art.walk(character, m, ColorTransform::default(), "", states, ovr, &mut out);
        for f in out {
            match f {
                // Mask layers (`mask_mc`, `_maskMenu_mc`, `maskContent`) clip in Flash; they are not drawn.
                Flat::Quad(q) if !is_mask(&q.path) => {
                    let Some(tex) = art.textures.get(&q.texture) else { continue };
                    let c = q.color;
                    let col = std::array::from_fn(|i| if i == 3 { (c.mul[3] + c.add[3]).clamp(0.0, 1.0) * alpha } else { (c.mul[i] + c.add[i]).clamp(0.0, 1.0) });
                    if col[3] <= 0.002 {
                        continue;
                    }
                    self.prims.push(Prim::Quad { tex: tex.clone(), src: q.src, dst: q.dst, color: col });
                }
                Flat::Solid(dst, c, path) if solid(&path) && !is_mask(&path) && c[3] * alpha > 0.002 => {
                    self.prims.push(Prim::Quad { tex: Handle::default(), src: [0.0, 0.0, 1.0, 1.0], dst, color: [c[0], c[1], c[2], c[3] * alpha] });
                }
                _ => {}
            }
        }
    }

    /// Solid-colour backdrops of `character` (axis-aligned fills; `alpha` scales them, as the
    /// movies fade these in from script).
    pub fn movie_solids(&mut self, art: &MovieArt, character: u16, m: Matrix, states: &States, alpha: f32) {
        let mut out = Vec::new();
        art.walk(character, m, ColorTransform::default(), "", states, &|_| None, &mut out);
        for f in out {
            let Flat::Solid(dst, c, _) = f else { continue };
            if c[3] <= 0.001 {
                continue;
            }
            let xs = dst.iter().map(|p| p.0);
            let ys = dst.iter().map(|p| p.1);
            let r = Rect::new(xs.clone().fold(f32::MAX, f32::min), ys.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max), ys.fold(f32::MIN, f32::max));
            self.prims.push(Prim::Fill { rect: r, color: Color::srgba(c[0], c[1], c[2], c[3] * alpha) });
        }
    }

    /// Text fields reachable from `character` (for placing strings into the movie's fields).
    pub fn fields(art: &MovieArt, character: u16, m: Matrix, states: &States) -> Vec<TextPlacement> {
        Self::fields_fx(art, character, m, states, &|_| None)
    }

    /// [`DisplayList::fields`] with clip property overrides.
    pub fn fields_fx(art: &MovieArt, character: u16, m: Matrix, states: &States, ovr: Ovr) -> Vec<TextPlacement> {
        let mut out = Vec::new();
        art.walk(character, m, ColorTransform::default(), "", states, ovr, &mut out);
        out.into_iter().filter_map(|f| if let Flat::Text(t) = f { Some(t) } else { None }).collect()
    }

    /// The label field of a list of fields: the first that is not a glow / shadow copy.
    pub fn label_field(fields: &[TextPlacement]) -> Option<&TextPlacement> {
        fields.iter().find(|f| !f.path.contains("Glow") && !f.path.contains("Shadow")).or(fields.first())
    }

    /// Draw `text` into a movie text field (font, size, colour and alignment from the field;
    /// fields sized for a placeholder grow around their anchor like `autoSize`). The field's
    /// accumulated alpha (its clips' `_alpha`) multiplies the colour.
    pub fn field_text(&mut self, art: &MovieArt, f: &TextPlacement, text: &str, color: Option<Color>) {
        let (x0, y0, x1, y1) = f.stage_rect();
        let scale = (f.matrix.a * f.matrix.a + f.matrix.b * f.matrix.b).sqrt().max(0.01);
        let size = f.text.height.max(8.0) * scale;
        let font = match f.text.font.and_then(|id| art.movie.font_name(id)) {
            Some(n) if n.contains("Title") || n.starts_with("Chalet") => FontKind::Title,
            _ => FontKind::Normal,
        };
        let align = match f.text.align {
            1 => Justify::Right,
            2 => Justify::Center,
            _ => Justify::Left,
        };
        // Grow narrow placeholder fields symmetrically (centre), rightwards (left) or leftwards
        // (right); multiline fields keep their box and wrap.
        // Flagged multiline but only one line tall (most labels): treat as single line.
        let multi = f.text.multiline && (f.text.bounds.ymax - f.text.bounds.ymin) / 20.0 > f.text.height * 1.8;
        let w = if multi { x1 - x0 } else { (x1 - x0).max(text.chars().count() as f32 * size * 0.62) };
        // Anchor on the field's own mid line (rotated fields: the text starts where the field
        // starts, not at the centre of its bounding box).
        let b = f.text.bounds;
        let ax = match align {
            Justify::Center => (b.xmin + b.xmax) * 0.5,
            Justify::Right => b.xmax,
            _ => b.xmin,
        };
        let (axs, ays) = f.matrix.apply(ax, (b.ymin + b.ymax) * 0.5);
        let (ax, ay) = (axs / 20.0, ays / 20.0);
        let (l, r) = match align {
            Justify::Center => (ax - w * 0.5, ax + w * 0.5),
            Justify::Right => (ax - w, ax),
            _ => (ax, ax + w),
        };
        let (y0, y1) = if multi { (y0, y1) } else { (ay - size * 0.65, ay + size * 0.65) };
        let m = f.color;
        let field_alpha = (m.mul[3] + m.add[3]).clamp(0.0, 1.0);
        let col = match color {
            Some(c) => c.with_alpha(c.alpha() * field_alpha),
            None => {
                let c = f.text.color.unwrap_or([227, 242, 214, 255]);
                Color::srgba(
                    (c[0] as f32 / 255.0 * m.mul[0] + m.add[0]).clamp(0.0, 1.0),
                    (c[1] as f32 / 255.0 * m.mul[1] + m.add[1]).clamp(0.0, 1.0),
                    (c[2] as f32 / 255.0 * m.mul[2] + m.add[2]).clamp(0.0, 1.0),
                    (c[3] as f32 / 255.0 * m.mul[3] + m.add[3]).clamp(0.0, 1.0),
                )
            }
        };
        if col.alpha() <= 0.002 {
            return;
        }
        // Rotated single-line fields: lay the text out in the field's own frame and rotate it.
        let angle = f.matrix.b.atan2(f.matrix.a);
        if !multi && angle.abs() > 0.03 {
            let (bx0, bx1) = (b.xmin / 20.0, b.xmax / 20.0);
            let wl = (bx1 - bx0).max(text.chars().count() as f32 * f.text.height * 0.62);
            let (lx0, lx1) = match align {
                Justify::Center => ((bx0 + bx1) * 0.5 - wl * 0.5, (bx0 + bx1) * 0.5 + wl * 0.5),
                Justify::Right => (bx1 - wl, bx1),
                _ => (bx0, bx0 + wl),
            };
            let (cx, cy) = f.matrix.apply((lx0 + lx1) * 10.0, (b.ymin + b.ymax) * 0.5);
            let rect = Rect::from_center_size(Vec2::new(cx, cy) / 20.0, Vec2::new(wl * scale, size * 1.3));
            self.prims.push(Prim::Text { text: text.to_string(), rect, size, font, color: col, align, top: false, rot: angle });
            return;
        }
        self.prims.push(Prim::Text { text: text.to_string(), rect: Rect::new(l, y0, r, y1.max(y0 + size * 1.2)), size, font, color: col, align, top: multi, rot: 0.0 });
    }

    pub fn text(&mut self, text: impl Into<String>, rect: Rect, size: f32, font: FontKind, color: Color, align: Justify) {
        self.prims.push(Prim::Text { text: text.into(), rect, size, font, color, align, top: false, rot: 0.0 });
    }

    /// Wrapped text block from the top of `rect`.
    pub fn paragraph(&mut self, text: impl Into<String>, rect: Rect, size: f32, font: FontKind, color: Color, align: Justify) {
        self.prims.push(Prim::Text { text: text.into(), rect, size, font, color, align, top: true, rot: 0.0 });
    }

    pub fn fill(&mut self, rect: Rect, color: Color) {
        self.prims.push(Prim::Fill { rect, color });
    }

    pub fn item(&mut self, id: impl Into<String>, rect: Rect, nav: bool) {
        self.items.push(Item { id: id.into(), rect, nav });
    }
}

/// Root of a menu layer's UI entities.
#[derive(Component)]
pub struct MenuLayer {
    /// Draw order of the layer (also its `GlobalZIndex`).
    #[allow(dead_code)]
    pub z: i32,
}

/// Stage geometry of one spawned primitive (positioned by [`layout_prims`]).
#[derive(Component, Clone)]
pub enum StagePrim {
    Quad([(f32, f32); 4]),
    Text { rect: Rect, size: f32, top: bool, rot: f32 },
    Fill(Rect),
}

/// Fonts for menu text: the game's own when `GameFonts` has loaded, Bevy's default otherwise.
pub fn font_handle(fonts: Option<&crate::gfx_hud::GameFonts>, kind: FontKind) -> Option<Handle<Font>> {
    fonts.map(|f| match kind {
        FontKind::Normal => f.normal.clone(),
        FontKind::Title => f.title.clone(),
    })
}

/// What must match for a spawned primitive to be updated in place.
#[derive(Clone, PartialEq)]
enum Sig {
    Quad(AssetId<Image>),
    Text(String, u32, FontKind, u8, bool),
    Fill,
}

fn sig(p: &Prim) -> Sig {
    match p {
        Prim::Quad { tex, .. } => Sig::Quad(tex.id()),
        Prim::Text { text, size, font, align, top, .. } => Sig::Text(text.clone(), (size * 100.0) as u32, *font, *align as u8, *top),
        Prim::Fill { .. } => Sig::Fill,
    }
}

/// The entities a layer last spawned, so an animated rebuild can update them in place.
#[derive(Default)]
pub struct LayerCache {
    kids: Vec<(Entity, Sig)>,
    fonts: bool,
}

/// Replace the children of `root` with the display list's primitives.
pub fn spawn_list(commands: &mut Commands, root: Entity, list: &DisplayList, fonts: Option<&crate::gfx_hud::GameFonts>) {
    let mut c = LayerCache::default();
    spawn_list_cached(commands, root, list, fonts, &mut c);
}

/// [`spawn_list`], updating the previous entities in place when the list has the same
/// primitives (same textures / texts) in the same order: during tweens only geometry and
/// colours change, and re-spawning would re-shape every text each frame.
pub fn spawn_list_cached(commands: &mut Commands, root: Entity, list: &DisplayList, fonts: Option<&crate::gfx_hud::GameFonts>, cache: &mut LayerCache) {
    let sigs: Vec<Sig> = list.prims.iter().map(sig).collect();
    let same = cache.fonts == fonts.is_some() && cache.kids.len() == sigs.len() && cache.kids.iter().zip(&sigs).all(|(k, s)| k.1 == *s);
    if same {
        for ((e, _), p) in cache.kids.iter().zip(&list.prims) {
            match p {
                Prim::Quad { tex, src, dst, color } => {
                    commands.entity(*e).insert((ImageNode { image: tex.clone(), rect: Some(Rect::new(src[0], src[1], src[2], src[3])), color: Color::srgba(color[0], color[1], color[2], color[3]), image_mode: NodeImageMode::Stretch, ..default() }, StagePrim::Quad(*dst)));
                }
                Prim::Text { rect, size, color, top, rot, .. } => {
                    commands.entity(*e).insert((TextColor(*color), StagePrim::Text { rect: *rect, size: *size, top: *top, rot: *rot }, UiTransform { rotation: Rot2::radians(*rot), ..default() }));
                }
                Prim::Fill { rect, color } => {
                    commands.entity(*e).insert((BackgroundColor(*color), StagePrim::Fill(*rect)));
                }
            }
        }
        return;
    }
    commands.entity(root).despawn_related::<Children>();
    cache.kids.clear();
    cache.fonts = fonts.is_some();
    for (p, s) in list.prims.iter().zip(sigs) {
        let e = match p {
            Prim::Quad { tex, src, dst, color } => commands
                .spawn((
                    ImageNode { image: tex.clone(), rect: Some(Rect::new(src[0], src[1], src[2], src[3])), color: Color::srgba(color[0], color[1], color[2], color[3]), image_mode: NodeImageMode::Stretch, ..default() },
                    Node { position_type: PositionType::Absolute, ..default() },
                    UiTransform::default(),
                    StagePrim::Quad(*dst),
                    Pickable::IGNORE,
                    ChildOf(root),
                ))
                .id(),
            Prim::Text { text, rect, size, font, color, align, top, rot } => {
                if std::env::var_os("DISHONORED_MENU_DEBUG").is_some() {
                    eprintln!("menu text {text:?} rect {rect:?} size {size} font {font:?}");
                }
                let mut tf = TextFont { font_size: FontSize::Px(*size), ..default() };
                if let Some(h) = font_handle(fonts, *font) {
                    tf.font = h.into();
                }
                commands
                    .spawn((
                        Text::new(text.clone()),
                        tf,
                        TextColor(*color),
                        TextLayout::justify(*align),
                        Node { position_type: PositionType::Absolute, ..default() },
                        StagePrim::Text { rect: *rect, size: *size, top: *top, rot: *rot },
                        UiTransform { rotation: Rot2::radians(*rot), ..default() },
                        Pickable::IGNORE,
                        ChildOf(root),
                    ))
                    .id()
            }
            Prim::Fill { rect, color } => commands.spawn((BackgroundColor(*color), Node { position_type: PositionType::Absolute, ..default() }, StagePrim::Fill(*rect), Pickable::IGNORE, ChildOf(root))).id(),
        };
        cache.kids.push((e, s));
    }
}

/// Stage -> window transform (uniform scale, centred: `gfx_stage::MovieSpace`).
pub fn stage_xform(window: Vec2) -> (f32, Vec2) {
    crate::gfx_stage::stage_xform(window)
}

/// Window cursor -> stage coordinates.
pub fn cursor_stage(window: &Window) -> Option<Vec2> {
    let (s, off) = stage_xform(Vec2::new(window.width(), window.height()));
    window.cursor_position().map(|c| (c - off) / s)
}

/// Fit every menu primitive to the current window.
pub fn layout_prims(windows: Query<&Window, With<bevy::window::PrimaryWindow>>, mut q: Query<(&StagePrim, &mut Node, Option<&mut UiTransform>, Option<&mut ImageNode>, Option<&mut TextFont>)>) {
    let Some(window) = windows.iter().next() else { return };
    let (s, off) = stage_xform(Vec2::new(window.width(), window.height()));
    for (p, mut node, xf, img, font) in &mut q {
        match p {
            StagePrim::Quad(dst) => {
                let pt = |c: (f32, f32)| Vec2::new(c.0, c.1) * s + off;
                let (tl, tr, br, bl) = (pt(dst[0]), pt(dst[1]), pt(dst[2]), pt(dst[3]));
                let e1 = tr - tl;
                let e2 = bl - tl;
                let mirrored = e1.x * e2.y - e1.y * e2.x < 0.0;
                let (w, h) = (e1.length(), e2.length());
                let c = (tl + br) * 0.5;
                // Written only on change: a changed node re-runs the UI layout (gfx_hud::place).
                crate::gfx_hud::place(&mut node, px(c.x - w * 0.5), px(c.y - h * 0.5), Some(px(w)), Some(px(h)));
                let dir = if mirrored { -e1 } else { e1 };
                if let Some(mut xf) = xf {
                    let r = Rot2::radians(dir.y.atan2(dir.x));
                    if xf.rotation != r {
                        xf.rotation = r;
                    }
                }
                if let Some(mut img) = img {
                    if img.flip_x != mirrored {
                        img.flip_x = mirrored;
                    }
                }
            }
            StagePrim::Text { rect, size, top, rot } => {
                if *rot != 0.0 {
                    // Rotated label: a fixed box rotated about its centre.
                    let min = rect.min * s + off;
                    let max = rect.max * s + off;
                    crate::gfx_hud::place(&mut node, px(min.x), px(min.y), Some(px(max.x - min.x)), Some(px(max.y - min.y)));
                    if let Some(mut f) = font {
                        let want = FontSize::Px((size * s).max(1.0));
                        if f.font_size != want {
                            f.font_size = want;
                        }
                    }
                    continue;
                }
                let min = rect.min * s + off;
                let max = rect.max * s + off;
                // Vertically centred in the field.
                let y = if *top { min.y } else { (min.y + max.y) * 0.5 - size * s * 0.62 };
                crate::gfx_hud::place(&mut node, px(min.x), px(y), Some(px((max.x - min.x).max(1.0))), None);
                if let Some(mut f) = font {
                    let want = FontSize::Px((size * s).max(1.0));
                    if f.font_size != want {
                        f.font_size = want;
                    }
                }
            }
            StagePrim::Fill(r) => {
                let min = r.min * s + off;
                let max = r.max * s + off;
                crate::gfx_hud::place(&mut node, px(min.x), px(min.y), Some(px(max.x - min.x)), Some(px(max.y - min.y)));
            }
        }
    }
}

/// Spawn a full-window layer root.
pub fn spawn_layer(commands: &mut Commands, z: i32) -> Entity {
    commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, GlobalZIndex(z), Pickable::IGNORE, MenuLayer { z })).id()
}
