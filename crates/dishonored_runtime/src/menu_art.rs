//! Drawing Dishonored's Scaleform menu movies with Bevy UI.
//!
//! A menu screen is rebuilt into a [`DisplayList`] of primitives in movie stage coordinates
//! (1280x720): textured quads flattened from the movie's sprites (atlas sub-rectangle + stage
//! corners, at the frame of a chosen state label such as `over`), text in the game's fonts
//! laid out in the movie's own text-field rectangles, and hit rectangles for interaction.
//! [`spawn_list`] turns the list into UI entities; [`layout_prims`] fits the stage to the
//! window every frame ("show all", centred), like `gfx_hud`.
//!
//! ActionScript is not executed: what the movie's classes do at runtime (attach list items,
//! fill text fields, play state transitions) is reproduced by the screen builders.

use crate::map_render::to_image;
use bevy::prelude::*;
use dishonored_assets::swf::{ColorTransform, Matrix, Movie, Quad, Sprite, TextPlacement};
use std::collections::HashMap;
use std::sync::Arc;
use ue3_package::ObjRef;
use ue3_package::properties::find;

pub const STAGE: Vec2 = Vec2::new(1280.0, 720.0);

/// A parsed movie plus its external textures, loaded once.
pub struct MovieArt {
    pub movie: Movie,
    pub textures: HashMap<String, Handle<Image>>,
}

fn movie_raw(pkg: &ue3_package::Package, name: &str) -> Option<Vec<u8>> {
    let i = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SwfMovie" && pkg.obj_path(ObjRef::Export(i)).ends_with(name))?;
    let h = pkg.read_object(i).ok()?;
    let (n, raw) = find(&h.properties, "RawData").and_then(|p| p.array_raw())?;
    Some(raw[..n].to_vec())
}

fn texture_index(pkg: &ue3_package::Package, name: &str) -> Option<usize> {
    let want = [name.to_string(), name.trim_end_matches(" -nopack").to_string()];
    (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "Texture2D" && pkg.obj_path(ObjRef::Export(i)).rsplit('.').next().is_some_and(|n| want.iter().any(|w| n.eq_ignore_ascii_case(w))))
}

impl MovieArt {
    /// Parse `movie` from `package` and decode every external image it names.
    pub fn load(content: &dishonored_assets::Content, package: &str, movie: &str, images: &mut Assets<Image>) -> Option<Self> {
        let pkg: Arc<ue3_package::Package> = content.package(package).ok()?;
        let movie = Movie::parse(&movie_raw(&pkg, movie)?).ok()?;
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
        Some(Self { movie, textures })
    }

    pub fn export(&self, name: &str) -> Option<u16> {
        self.movie.exports.get(name).copied()
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

    pub fn sprite(&self, id: u16) -> Option<&Sprite> {
        self.movie.sprites.get(&id)
    }

    /// Stage bounds (pixels) of a character flattened at `m`.
    pub fn bounds(&self, character: u16, m: Matrix, states: &States) -> Option<Rect> {
        let mut qs = Vec::new();
        self.movie.flatten(character, m, ColorTransform::default(), "", &|p, s| states.frame(p, s), &mut qs);
        let mut r: Option<Rect> = None;
        for q in &qs {
            for c in q.dst {
                let p = Vec2::new(c.0, c.1);
                r = Some(r.map_or(Rect::from_corners(p, p), |r| r.union_point(p)));
            }
        }
        r
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

/// End frame of a state label's transition (the frame before the next label), so a static
/// rebuild shows the settled look of `over`, `selected`, ...
pub fn state_frame(sprite: &Sprite, label: &str) -> usize {
    let labels = Movie::labels(sprite);
    match labels.iter().position(|(_, l)| l.eq_ignore_ascii_case(label)) {
        Some(i) => labels.get(i + 1).map(|(f, _)| f.saturating_sub(1)).unwrap_or(sprite.frames.len().saturating_sub(1)).max(labels[i].0),
        None => 0,
    }
}

/// Which state label to show per placement path (exact path, or `*` for every sprite that
/// has the label).
#[derive(Default, Clone)]
pub struct States(pub Vec<(String, String)>);

impl States {
    pub fn all(label: &str) -> Self {
        Self(vec![("*".into(), label.into())])
    }
    pub fn with(mut self, path: &str, label: &str) -> Self {
        self.0.push((path.into(), label.into()));
        self
    }
    pub fn frame(&self, path: &str, s: &Sprite) -> usize {
        for (p, l) in self.0.iter().rev() {
            // "#N": a fixed frame (sprites whose frame the script picks, e.g. random strip variants).
            if let Some(n) = l.strip_prefix('#').and_then(|n| n.parse::<usize>().ok()) {
                if p == path || p == "*" {
                    return n.min(s.frames.len().saturating_sub(1));
                }
                continue;
            }
            if (p == "*" || p == path) && Movie::label_frame(s, l).is_some() {
                return state_frame(s, l);
            }
        }
        0
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FontKind {
    /// `$NormalFont` ("Emerge BF").
    Normal,
    /// `$TitleFont` ("ChaletComprime-CologneEighty").
    Title,
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
pub const TEXT_DARK: Color = Color::srgb(23.0 / 255.0, 25.0 / 255.0, 28.0 / 255.0);

pub fn px_matrix(x: f32, y: f32) -> Matrix {
    Matrix { tx: x * 20.0, ty: y * 20.0, ..Default::default() }
}

impl DisplayList {
    /// Flatten `character` at stage matrix `m` (twips) into quads.
    pub fn movie(&mut self, art: &MovieArt, character: u16, m: Matrix, states: &States, alpha: f32) -> Vec<Quad> {
        let mut qs = Vec::new();
        art.movie.flatten(character, m, ColorTransform::default(), "", &|p, s| states.frame(p, s), &mut qs);
        // Mask layers (`mask_mc`, `_maskMenu_mc`, `maskContent`) clip in Flash; they are not drawn.
        qs.retain(|q| !q.path.to_ascii_lowercase().contains("mask"));
        if std::env::var_os("DISHONORED_MENU_DEBUG").is_some() {
            for q in &qs {
                eprintln!("menu quad char {character} path {:?} tex {} dst {:?}", q.path, q.texture, q.dst[0]);
            }
        }
        for q in &qs {
            let Some(tex) = art.textures.get(&q.texture) else { continue };
            let c = q.color;
            let col = std::array::from_fn(|i| if i == 3 { (c.mul[3] + c.add[3]).clamp(0.0, 1.0) * alpha } else { (c.mul[i] + c.add[i]).clamp(0.0, 1.0) });
            self.prims.push(Prim::Quad { tex: tex.clone(), src: q.src, dst: q.dst, color: col });
        }
        qs
    }

    /// Solid-colour backdrops of `character` (axis-aligned fills; `alpha` scales them, as the
    /// movies fade these in from script).
    pub fn movie_solids(&mut self, art: &MovieArt, character: u16, m: Matrix, states: &States, alpha: f32) {
        let mut out = Vec::new();
        art.movie.flatten_solids(character, m, ColorTransform::default(), "", &|p, s| states.frame(p, s), &mut out);
        for (dst, c, _) in out {
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
        let mut out = Vec::new();
        art.movie.flatten_texts(character, m, ColorTransform::default(), "", &|p, s| states.frame(p, s), &mut out);
        out
    }

    /// Draw `text` into a movie text field (font, size, colour and alignment from the field;
    /// fields sized for a placeholder grow around their anchor like `autoSize`).
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
        let col = color.unwrap_or_else(|| {
            let c = f.text.color.unwrap_or([227, 242, 214, 255]);
            let m = f.color;
            Color::srgba(
                (c[0] as f32 / 255.0 * m.mul[0] + m.add[0]).clamp(0.0, 1.0),
                (c[1] as f32 / 255.0 * m.mul[1] + m.add[1]).clamp(0.0, 1.0),
                (c[2] as f32 / 255.0 * m.mul[2] + m.add[2]).clamp(0.0, 1.0),
                (c[3] as f32 / 255.0 * m.mul[3] + m.add[3]).clamp(0.0, 1.0),
            )
        });
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

/// Replace the children of `root` with the display list's primitives.
pub fn spawn_list(commands: &mut Commands, root: Entity, list: &DisplayList, fonts: Option<&crate::gfx_hud::GameFonts>) {
    commands.entity(root).despawn_related::<Children>();
    for p in &list.prims {
        match p {
            Prim::Quad { tex, src, dst, color } => {
                commands.spawn((
                    ImageNode { image: tex.clone(), rect: Some(Rect::new(src[0], src[1], src[2], src[3])), color: Color::srgba(color[0], color[1], color[2], color[3]), ..default() },
                    Node { position_type: PositionType::Absolute, ..default() },
                    UiTransform::default(),
                    StagePrim::Quad(*dst),
                    Pickable::IGNORE,
                    ChildOf(root),
                ));
            }
            Prim::Text { text, rect, size, font, color, align, top, rot } => {
                if std::env::var_os("DISHONORED_MENU_DEBUG").is_some() {
                    eprintln!("menu text {text:?} rect {rect:?} size {size} font {font:?}");
                }
                let mut tf = TextFont { font_size: FontSize::Px(*size), ..default() };
                if let Some(h) = font_handle(fonts, *font) {
                    tf.font = h.into();
                }
                commands.spawn((
                    Text::new(text.clone()),
                    tf,
                    TextColor(*color),
                    TextLayout::justify(*align),
                    Node { position_type: PositionType::Absolute, ..default() },
                    StagePrim::Text { rect: *rect, size: *size, top: *top, rot: *rot },
                    UiTransform { rotation: Rot2::radians(*rot), ..default() },
                    Pickable::IGNORE,
                    ChildOf(root),
                ));
            }
            Prim::Fill { rect, color } => {
                commands.spawn((BackgroundColor(*color), Node { position_type: PositionType::Absolute, ..default() }, StagePrim::Fill(*rect), Pickable::IGNORE, ChildOf(root)));
            }
        }
    }
}

/// Stage -> window transform (uniform scale, centred).
pub fn stage_xform(window: Vec2) -> (f32, Vec2) {
    let s = (window.x / STAGE.x).min(window.y / STAGE.y);
    (s, (window - STAGE * s) * 0.5)
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
                node.left = px(c.x - w * 0.5);
                node.top = px(c.y - h * 0.5);
                node.width = px(w);
                node.height = px(h);
                let dir = if mirrored { -e1 } else { e1 };
                if let Some(mut xf) = xf {
                    xf.rotation = Rot2::radians(dir.y.atan2(dir.x));
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
                    node.left = px(min.x);
                    node.top = px(min.y);
                    node.width = px(max.x - min.x);
                    node.height = px(max.y - min.y);
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
                node.left = px(min.x);
                node.width = px((max.x - min.x).max(1.0));
                // Vertically centred in the field.
                node.top = px(if *top { min.y } else { (min.y + max.y) * 0.5 - size * s * 0.62 });
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
                node.left = px(min.x);
                node.top = px(min.y);
                node.width = px(max.x - min.x);
                node.height = px(max.y - min.y);
            }
        }
    }
}

/// Spawn a full-window layer root.
pub fn spawn_layer(commands: &mut Commands, z: i32) -> Entity {
    commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, GlobalZIndex(z), Pickable::IGNORE, MenuLayer { z })).id()
}
