//! In-game HUD rebuilt from Dishonored's own Scaleform movie (`UI_HUD_SF` `HUD`), loaded from
//! the install at runtime: the movie is flattened into textured quads (atlas sub-rectangle +
//! stage transform) and drawn as Bevy UI images, scaled from the 1280x720 stage.
//!
//! ActionScript is not run. The pieces it drives are reproduced from simulation state:
//! health/mana fills are cropped along the shard like the movie's `mask_mc`, crosshair variants
//! follow the left-hand item, the crouch icon follows stance, and the equipment emblem shows
//! the left-hand item's icon (`UI_ItemIcons_Large`, `UI_Powers_Large`) with its ammo.

use crate::map_render::to_image;
use crate::session::{Session, SessionView};
use bevy::prelude::*;
use dishonored_assets::swf::{ColorTransform, Matrix, Movie, Quad};
use dishonored_sim::input::LeftHand;
use dishonored_sim::state::LeftPhase;
use std::collections::HashMap;
use ue3_package::ObjRef;
use ue3_package::properties::find;

const STAGE: Vec2 = Vec2::new(1280.0, 720.0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Group {
    Static,
    HealthFill,
    HealthGhost,
    ManaFill,
    ManaGhost,
    CrossDot,
    CrossGun,
    CrossCrossbow,
    CrossPower,
    Crouch,
    /// Objectives banner (top right): match mode, clock, score.
    Objectives,
    /// Target notification paper (top right, below): Contracts target clues.
    TargetNote,
    /// Mission objective marker (`objectiveMarker_primary`), projected on the task target.
    Marker,
    /// Backdrop of the end-of-mission panel (`hud_obj_bkgdR`).
    OutcomeBg,
}

#[derive(Component)]
pub struct HudQuad {
    group: Group,
    quad: Quad,
}

#[derive(Component)]
pub struct EquipIcon {
    /// Stage-space centre and size.
    center: Vec2,
    size: f32,
}

#[derive(Component)]
pub struct AmmoText {
    pos: Vec2,
}

#[derive(Resource, Default)]
pub struct GfxHud {
    icons: HashMap<&'static str, Handle<Image>>,
    pub loaded: bool,
}

/// Left-hand item -> icon texture (package, Texture2D name).
const ICONS: &[(&str, &str, &str)] = &[
    ("blink", "UI_Powers_Large", "BlinkBig"),
    ("windblast", "UI_Powers_Large", "WindblastBig"),
    ("darkvision", "UI_Powers_Large", "DarkVisionBig"),
    ("pistol", "UI_ItemIcons_Large", "Gun_Large"),
    ("crossbow", "UI_ItemIcons_Large", "Crossbow_Large"),
    ("sword", "UI_ItemIcons_Large", "CorvoSword_Large"),
];

fn movie_raw(pkg: &ue3_package::Package, name: &str) -> Option<Vec<u8>> {
    let i = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SwfMovie" && pkg.obj_path(ObjRef::Export(i)).ends_with(name))?;
    let h = pkg.read_object(i).ok()?;
    let (n, raw) = find(&h.properties, "RawData").and_then(|p| p.array_raw())?;
    Some(raw[..n].to_vec())
}

fn texture(content: &dishonored_assets::Content, pkg: &ue3_package::Package, name: &str, images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let ti = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "Texture2D" && pkg.obj_path(ObjRef::Export(i)).rsplit('.').next().is_some_and(|n| n.eq_ignore_ascii_case(name)))?;
    let t = dishonored_assets::texture::read_texture(content, pkg, ti, 1024).ok()?;
    Some(images.add(to_image(&t)))
}

pub fn setup_gfx_hud(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut hud = GfxHud::default();
    let Ok(content) = dishonored_assets::Content::locate() else { return commands.insert_resource(hud) };
    let Ok(pkg) = content.package("UI_HUD_SF") else { return commands.insert_resource(hud) };
    let Some(movie) = movie_raw(&pkg, "HUD").and_then(|r| Movie::parse(&r).ok()) else {
        warn!("HUD movie not readable");
        return commands.insert_resource(hud);
    };
    let root_list = movie.display_list(&movie.root, 0);
    let placed = |name: &str| root_list.iter().find(|p| p.name.as_deref() == Some(name)).cloned();
    let frame0 = |_: &str, _: &dishonored_assets::swf::Sprite| 0usize;
    let mut pieces: Vec<(Group, Quad)> = Vec::new();
    // Health / mana gauges.
    if let Some(p) = placed("masterHUD_mc") {
        let mut qs = Vec::new();
        movie.flatten(p.character.unwrap_or(0), p.matrix.unwrap_or_default(), ColorTransform::default(), "masterHUD_mc", &frame0, &mut qs);
        for q in qs {
            // Script-driven flashes are not reproduced.
            if q.path.contains("refill_mc") || q.path.contains("indic_mc") {
                continue;
            }
            let ghost = q.color.mul[0] < 0.99;
            let g = match (q.path.contains("_healthMeter_mc"), q.path.contains("_manaMeter_mc"), q.character) {
                (true, _, 642) => if ghost { Group::HealthGhost } else { Group::HealthFill },
                (_, true, 682) => if ghost { Group::ManaGhost } else { Group::ManaFill },
                _ => Group::Static,
            };
            pieces.push((g, q));
        }
    }
    // Crosshairs: the placed dot plus the exported variants, all at screen centre.
    let centre = placed("_dot_mc").and_then(|p| p.matrix).unwrap_or(Matrix { tx: 640.0 * 20.0, ty: 360.0 * 20.0, ..Default::default() });
    for (export, g) in [("crosshair_dot", Group::CrossDot), ("crosshair_gun", Group::CrossGun), ("crosshair_crossbow", Group::CrossCrossbow), ("crosshair_power", Group::CrossPower)] {
        if let Some(id) = movie.exports.get(export) {
            let mut qs = Vec::new();
            movie.flatten(*id, centre, ColorTransform::default(), export, &frame0, &mut qs);
            pieces.extend(qs.into_iter().map(|q| (g, q)));
        }
    }
    // Objectives banner and target notification (top right) carry match information.
    for (name, g) in [("objectives_mc", Group::Objectives), ("targetNotification_mc", Group::TargetNote)] {
        if let Some(p) = placed(name) {
            let mut qs = Vec::new();
            movie.flatten(p.character.unwrap_or(0), p.matrix.unwrap_or_default(), ColorTransform::default(), name, &frame0, &mut qs);
            // Decorative schema lines and the animated portrait stay out.
            // In the game the two never show together; here the paper sits under the banner.
            let dy = if g == Group::TargetNote { 120.0 } else { 0.0 };
            pieces.extend(qs.into_iter().filter(|q| !q.path.contains("schema") && !q.path.contains("animPortrait") && !q.path.contains("objState")).map(|mut q| {
                q.dst = q.dst.map(|(x, y)| (x, y + dy));
                (g, q)
            }));
        }
    }
    // Stance icon (bottom left).
    if let (Some(p), Some(id)) = (placed("playerStatesIcon_mc"), movie.exports.get("crouch")) {
        let mut qs = Vec::new();
        movie.flatten(*id, p.matrix.unwrap_or_default(), ColorTransform::default(), "crouch", &frame0, &mut qs);
        pieces.extend(qs.into_iter().map(|q| (Group::Crouch, q)));
    }
    // Objective marker sprite at the stage origin (moved per frame) and the outcome backdrop.
    if let Some(id) = movie.exports.get("objectiveMarker_primary") {
        let mut qs = Vec::new();
        movie.flatten(*id, Matrix::default(), ColorTransform::default(), "marker", &frame0, &mut qs);
        pieces.extend(qs.into_iter().map(|q| (Group::Marker, q)));
    }
    if let Some(img) = movie.images.values().find(|i| i.export_name.starts_with("hud_obj_bkgdR")) {
        let tex = img.file_name.trim_end_matches(".tga").to_string();
        pieces.push((Group::OutcomeBg, Quad { texture: tex, src: [0.0, 0.0, img.width as f32, img.height as f32], dst: [(290.0, 190.0), (990.0, 190.0), (990.0, 470.0), (290.0, 470.0)], color: ColorTransform { mul: [1.0, 1.0, 1.0, 0.92], add: [0.0; 4] }, path: "outcome".into(), character: 0 }));
    }
    // Equipment emblem position (masterHUD_mc/_equipment_mc).
    let equip = placed("masterHUD_mc").and_then(|p| {
        let sp = movie.sprites.get(&p.character?)?;
        let e = movie.display_list(sp, 0).into_iter().find(|c| c.name.as_deref() == Some("_equipment_mc"))?;
        let m = p.matrix.unwrap_or_default().mul(&e.matrix.unwrap_or_default());
        Some((Vec2::new(m.tx, m.ty) / 20.0, m.a))
    });
    // Text anchors: stage rectangles of the banner strips and the target paper.
    let rect_of = |pred: &dyn Fn(&Group, &Quad) -> bool| -> Option<Rect> {
        let mut r: Option<Rect> = None;
        for (g, q) in pieces.iter().filter(|(g, q)| pred(g, q)) {
            for c in q.dst {
                let p = Vec2::new(c.0, c.1);
                r = Some(match r {
                    Some(r) => r.union_point(p),
                    None => Rect::from_corners(p, p),
                });
            }
            let _ = g;
        }
        r
    };
    let anchors = [
        (TextKind::ObjTitle, rect_of(&|_, q| q.path.contains("title_mc"))),
        (TextKind::ObjName, rect_of(&|_, q| q.path.contains("name_mc"))),
        (TextKind::TargetBody, rect_of(&|g, _| *g == Group::TargetNote)),
        (TextKind::Message, Some(Rect::new(290.0, 600.0, 990.0, 640.0))),
        (TextKind::Outcome, Some(Rect::new(330.0, 225.0, 950.0, 440.0))),
    ];
    // Textures.
    let mut tex: HashMap<String, Handle<Image>> = HashMap::new();
    for (_, q) in &pieces {
        if !tex.contains_key(&q.texture) {
            if let Some(h) = texture(&content, &pkg, &q.texture, &mut images) {
                tex.insert(q.texture.clone(), h);
            }
        }
    }
    for (key, pkg_name, name) in ICONS {
        if let Some(h) = content.package(pkg_name).ok().and_then(|p| texture(&content, &p, name, &mut images)) {
            hud.icons.insert(key, h);
        }
    }
    let root = commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, GlobalZIndex(-1), Pickable::IGNORE)).id();
    for (g, q) in pieces {
        let Some(img) = tex.get(&q.texture) else { continue };
        let c = q.color;
        let color = Color::srgba(c.mul[0].clamp(0.0, 1.0), c.mul[1].clamp(0.0, 1.0), c.mul[2].clamp(0.0, 1.0), c.mul[3].clamp(0.0, 1.0));
        commands.spawn((
            ImageNode { image: img.clone(), rect: Some(Rect::new(q.src[0], q.src[1], q.src[2], q.src[3])), color, ..default() },
            Node { position_type: PositionType::Absolute, ..default() },
            UiTransform::default(),
            HudQuad { group: g, quad: q },
            ChildOf(root),
        ));
    }
    if let Some((pos, scale)) = equip {
        let size = 150.0 * scale;
        commands.spawn((ImageNode::default(), Node { position_type: PositionType::Absolute, ..default() }, Visibility::Hidden, EquipIcon { center: pos, size }, ChildOf(root)));
        commands.spawn((
            Text::new(""),
            TextFont { font_size: FontSize::Px(18.0), ..default() },
            TextColor(Color::srgb(0.93, 0.92, 0.85)),
            TextShadow::default(),
            Node { position_type: PositionType::Absolute, ..default() },
            AmmoText { pos: pos + Vec2::new(size * 0.45, size * 0.1) },
            ChildOf(root),
        ));
    }
    for (kind, rect) in anchors {
        let Some(rect) = rect else { continue };
        commands.spawn((
            Text::new(""),
            TextFont { font_size: FontSize::Px(16.0), ..default() },
            TextColor(if kind == TextKind::ObjTitle { Color::srgb(0.08, 0.08, 0.08) } else { Color::srgb(0.93, 0.91, 0.84) }),
            TextLayout::justify(if matches!(kind, TextKind::Message | TextKind::Outcome) { Justify::Center } else { Justify::Right }),
            Node { position_type: PositionType::Absolute, ..default() },
            AnchoredText { kind, rect },
            ChildOf(root),
        ));
    }
    hud.loaded = true;
    info!("HUD: {} quads from UI_HUD_SF", tex.len());
    commands.insert_resource(hud);
}

/// Stage -> window transform (uniform scale, centred; GFx "show all").
fn stage_xform(window: Vec2) -> (f32, Vec2) {
    let s = (window.x / STAGE.x).min(window.y / STAGE.y);
    (s, (window - STAGE * s) * 0.5)
}

/// Keep the bottom `f` of a quad (the shard grows away from the emblem, i.e. towards the
/// source's top edge), like the movie's sliding `mask_mc`.
fn crop(q: &Quad, f: f32) -> ([f32; 4], [(f32, f32); 4]) {
    let f = f.clamp(0.0, 1.0);
    let [x0, y0, x1, y1] = q.src;
    let src = [x0, y1 - (y1 - y0) * f, x1, y1];
    let lerp = |a: (f32, f32), b: (f32, f32)| (b.0 + (a.0 - b.0) * f, b.1 + (a.1 - b.1) * f);
    let [tl, tr, br, bl] = q.dst;
    (src, [lerp(tl, bl), lerp(tr, br), br, bl])
}

#[allow(clippy::too_many_arguments)]
pub fn update_gfx_hud(
    hud: Option<Res<GfxHud>>,
    view: Res<SessionView>,
    session: Res<Session>,
    windows: Query<&Window>,
    mut quads: Query<(&HudQuad, &mut Node, &mut UiTransform, &mut ImageNode, &mut Visibility), (Without<EquipIcon>, Without<AmmoText>)>,
    mut icon: Query<(&EquipIcon, &mut Node, &mut ImageNode, &mut Visibility), (Without<HudQuad>, Without<AmmoText>)>,
    mut ammo: Query<(&AmmoText, &mut Node, &mut Text, &mut TextFont), (Without<HudQuad>, Without<EquipIcon>)>,
    cams: Query<(&Camera, &GlobalTransform), With<crate::players::GameCamera>>,
) {
    let Some(hud) = hud.filter(|h| h.loaded) else { return };
    let Some(window) = windows.iter().next() else { return };
    let (s, off) = stage_xform(Vec2::new(window.width(), window.height()));
    let me = view.me.as_ref();
    let vit = &session.content.sim.gameplay.vitals;
    let alive = me.is_some_and(|m| m.alive);
    let health = me.map(|m| m.vitals.health / vit.health_max.max(1.0)).unwrap_or(0.0);
    let mana = me.map(|m| m.vitals.mana / vit.mana_max.max(1.0)).unwrap_or(0.0);
    let mana_cap = me.map(|m| m.vitals.mana_regen_cap / vit.mana_max.max(1.0)).unwrap_or(0.0).max(mana);
    let item = me.map(|m| m.left.item).unwrap_or(LeftHand::Empty);
    let aiming_power = me.is_some_and(|m| m.left.phase == LeftPhase::Aiming && matches!(m.left.item, LeftHand::Blink));
    let crouched = me.is_some_and(|m| m.crouched);
    let has_match = !view.matchv.mode.is_empty();
    let mission = view.matchv.mission.as_ref();
    let ended = mission.is_some_and(|m| m.outcome.is_some());
    // Objective marker: active task target projected into the stage (stage units).
    let marker = mission.and_then(|m| m.target).filter(|_| !ended).and_then(|t| {
        let (cam, gt) = cams.iter().next()?;
        let world = crate::coords::pos(Vec3::from(t) + Vec3::Z * 60.0);
        let v = cam.world_to_viewport(gt, world).ok()?;
        Some((v - off) / s)
    });
    let has_match = !view.matchv.mode.is_empty();
    for (hq, mut node, mut xf, mut img, mut vis) in &mut quads {
        let (show, frac) = match hq.group {
            Group::Static => (alive, None),
            Group::HealthFill => (alive, Some(health)),
            Group::HealthGhost => (alive, Some(health)),
            Group::ManaFill => (alive, Some(mana)),
            Group::ManaGhost => (alive, Some(mana_cap)),
            Group::CrossDot => (alive && !aiming_power && !matches!(item, LeftHand::Pistol | LeftHand::Crossbow), None),
            Group::CrossGun => (alive && item == LeftHand::Pistol, None),
            Group::CrossCrossbow => (alive && item == LeftHand::Crossbow, None),
            Group::CrossPower => (alive && aiming_power, None),
            Group::Crouch => (alive && crouched, None),
            Group::Objectives => (has_match, None),
            Group::TargetNote => (view.target_info.is_some() || (mission.is_some() && !ended), None),
            Group::Marker => (alive && marker.is_some(), None),
            Group::OutcomeBg => (ended, None),
        };
        *vis = if show && frac.is_none_or(|f| f > 0.001) { Visibility::Inherited } else { Visibility::Hidden };
        let (src, dst) = match frac {
            Some(f) => crop(&hq.quad, f),
            None => (hq.quad.src, hq.quad.dst),
        };
        let dst = match (hq.group, marker) {
            (Group::Marker, Some(at)) => dst.map(|(x, y)| (x + at.x, y + at.y)),
            _ => dst,
        };
        img.rect = Some(Rect::new(src[0], src[1], src[2], src[3]));
        let p = |c: (f32, f32)| Vec2::new(c.0, c.1) * s + off;
        let (tl, tr, br, bl) = (p(dst[0]), p(dst[1]), p(dst[2]), p(dst[3]));
        let w = (tr - tl).length();
        let h = (bl - tl).length();
        let c = (tl + br) * 0.5;
        node.left = px(c.x - w * 0.5);
        node.top = px(c.y - h * 0.5);
        node.width = px(w);
        node.height = px(h);
        let ang = (tr - tl).y.atan2((tr - tl).x);
        xf.rotation = Rot2::radians(ang);
    }
    let key = match item {
        LeftHand::Blink => Some("blink"),
        LeftHand::Windblast => Some("windblast"),
        LeftHand::DarkVision => Some("darkvision"),
        LeftHand::Pistol => Some("pistol"),
        LeftHand::Crossbow => Some("crossbow"),
        LeftHand::Empty => Some("sword"),
    };
    for (ei, mut node, mut img, mut vis) in &mut icon {
        match key.and_then(|k| hud.icons.get(k)).filter(|_| alive) {
            Some(h) => {
                img.image = h.clone();
                *vis = Visibility::Inherited;
            }
            None => *vis = Visibility::Hidden,
        }
        let size = ei.size * s;
        let c = ei.center * s + off;
        node.left = px(c.x - size * 0.5);
        node.top = px(c.y - size * 0.5);
        node.width = px(size);
        node.height = px(size);
    }
    for (at, mut node, mut text, mut font) in &mut ammo {
        let p = at.pos * s + off;
        node.left = px(p.x);
        node.top = px(p.y);
        font.font_size = FontSize::Px(18.0 * s);
        text.0 = match (me, item) {
            (Some(m), LeftHand::Pistol) if alive => format!("{} | {}", m.left.pistol_loaded, m.left.pistol_reserve),
            (Some(m), LeftHand::Crossbow) if alive => format!("{} | {}", m.left.bolts_loaded, m.left.bolts_reserve),
            _ => String::new(),
        };
    }
}

/// Dishonored's UI typefaces, converted at startup from `DisFonts_SF` (`fonts_efigs`):
/// "Emerge BF" (body) and "ChaletComprime-CologneEighty" (titles).
#[derive(Resource, Clone)]
pub struct GameFonts {
    pub normal: Handle<Font>,
    pub title: Handle<Font>,
}

pub fn load_game_fonts(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    let Some(movie) = content.package("DisFonts_SF").ok().and_then(|p| movie_raw(&p, "fonts_efigs")).and_then(|r| Movie::parse(&r).ok()) else { return };
    let mut by_name = HashMap::new();
    for f in &movie.fonts {
        by_name.insert(f.name.clone(), fonts.add(Font::from_bytes(f.to_ttf(&f.name))));
    }
    let normal = by_name.iter().find(|(n, _)| n.starts_with("Emerge")).map(|(_, h)| h.clone());
    let title = by_name.iter().find(|(n, _)| n.starts_with("Chalet")).map(|(_, h)| h.clone());
    if let (Some(normal), Some(title)) = (normal, title) {
        info!("game fonts: {:?}", by_name.keys().collect::<Vec<_>>());
        commands.insert_resource(GameFonts { normal, title });
    }
}

/// Give HUD text the game's fonts once they exist (body font; scoreboard uses titles).
pub fn apply_game_fonts(
    fonts: Option<Res<GameFonts>>,
    mut done: Local<bool>,
    mut body: Query<&mut TextFont, (Or<(With<crate::hud::HudText>, With<AmmoText>, With<AnchoredText>)>, Without<crate::hud::Scoreboard>)>,
    mut titles: Query<&mut TextFont, (With<crate::hud::Scoreboard>, Without<crate::hud::HudText>, Without<AmmoText>, Without<AnchoredText>)>,
) {
    let Some(fonts) = fonts else { return };
    if *done || (body.is_empty() && titles.is_empty()) {
        return;
    }
    for mut f in &mut body {
        f.font = fonts.normal.clone().into();
    }
    for mut f in &mut titles {
        f.font = fonts.title.clone().into();
    }
    *done = true;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextKind {
    ObjTitle,
    ObjName,
    TargetBody,
    /// HUD message (bottom centre, where the movie places `gameMsg_mc`).
    Message,
    /// End-of-mission panel.
    Outcome,
}

/// Text laid out inside a stage-space rectangle of the movie.
#[derive(Component)]
pub struct AnchoredText {
    pub kind: TextKind,
    pub rect: Rect,
}

/// Contracts clues worded for the target paper (never coordinates).
fn target_lines(view: &SessionView, yaw: f32) -> String {
    use dishonored_multiplayer::contracts::{Proximity, PursuitAwareness};
    let Some(ti) = view.target_info.as_ref() else { return String::new() };
    let looks = match ti.appearance {
        Some(0) => "Masked gentleman, dark coat, metal mask",
        Some(2) => "Masked gentleman, shirtsleeves, metal mask",
        Some(3) => "Masked lady, black mask",
        Some(1) => "City watch uniform",
        _ => "Unknown",
    };
    let dir = ti.bearing_octant.map(|o| {
        let ang = (o as f32 * 45.0 - yaw.rem_euclid(360.0)).rem_euclid(360.0);
        ["ahead", "ahead, left", "to your left", "behind, left", "behind you", "behind, right", "to your right", "ahead, right"][(((ang + 22.5) / 45.0) as usize) % 8]
    });
    let dist = match ti.proximity {
        Some(Proximity::Far) => "far away",
        Some(Proximity::Medium) => "some distance off",
        Some(Proximity::Near) => "near",
        Some(Proximity::VeryNear) => "very near",
        None => "",
    };
    let mut s = format!("TARGET\n{looks}\n");
    s += &match dir {
        Some(d) => format!("{d}, {dist}\n"),
        None => "Reacquiring...\n".into(),
    };
    s += match ti.target_awareness {
        PursuitAwareness::Unaware => "Unaware of you",
        PursuitAwareness::Suspicious => "Suspicious",
        PursuitAwareness::Aware => "Aware of you",
        PursuitAwareness::Pursuit => "Fleeing",
    };
    s += "\n";
    if ti.being_chased {
        s += "You are being chased\n";
    } else if ti.pursuer_near {
        s += "Your hunter is near\n";
    }
    if let Some(t) = ti.escaping {
        s += &format!("Escape: {t:.0}s\n");
    }
    s
}

pub fn update_anchored_text(
    view: Res<SessionView>,
    local: Res<crate::input_map::LocalInput>,
    time: Res<Time>,
    mut seen: Local<HashMap<u32, f32>>,
    windows: Query<&Window>,
    mut texts: Query<(&AnchoredText, &mut Node, &mut Text, &mut TextFont, &mut Visibility)>,
) {
    let Some(window) = windows.iter().next() else { return };
    let (s, off) = stage_xform(Vec2::new(window.width(), window.height()));
    let m = &view.matchv;
    let me_id = view.me.as_ref().map(|p| p.id);
    let mine = m.scores.iter().find(|e| Some(e.id) == me_id);
    let leader = m.scores.iter().max_by_key(|e| e.score);
    for (at, mut node, mut text, mut font, mut vis) in &mut texts {
        let mission = m.mission.as_ref();
        let (content, size) = match (at.kind, mission) {
            (TextKind::ObjTitle, Some(mv)) => (mission_title(mv), 17.0),
            (TextKind::ObjName, Some(mv)) => (mission_task(mv), 15.0),
            (TextKind::TargetBody, Some(mv)) => (mission_paper(mv, time.elapsed_secs(), &mut seen), 15.0),
            (TextKind::ObjTitle, None) => (format!("{}   {}:{:02}", if m.mode.eq_ignore_ascii_case("contracts") { "CONTRACTS" } else { "FREE FOR ALL" }, (m.time_left / 60.0).floor() as i32, (m.time_left % 60.0) as i32), 17.0),
            (TextKind::ObjName, None) => (
                match (mine, leader) {
                    (Some(me), Some(l)) if l.id != me.id => format!("You {}   ·   {} leads with {}", me.score, l.name, l.score),
                    (Some(me), _) => format!("You lead with {}", me.score),
                    _ => String::new(),
                },
                15.0,
            ),
            (TextKind::TargetBody, None) => (target_lines(&view, local.yaw), 15.0),
            (TextKind::Message, Some(mv)) => (mission_message(mv, time.elapsed_secs(), &mut seen), 20.0),
            (TextKind::Outcome, Some(mv)) => (mission_outcome(mv), 19.0),
            (TextKind::Message | TextKind::Outcome, None) => (String::new(), 15.0),
        };
        *vis = if content.is_empty() || m.mode.is_empty() { Visibility::Hidden } else { Visibility::Inherited };
        text.0 = content;
        font.font_size = FontSize::Px(size * s);
        let min = at.rect.min * s + off;
        let max = at.rect.max * s + off;
        // Banner strips: right-aligned inside the strip; target paper: inset block.
        let (inset, top) = match at.kind {
            TextKind::TargetBody => (Vec2::new(0.1, 0.0), min.y + (max.y - min.y) * 0.3),
            TextKind::Message | TextKind::Outcome => (Vec2::new(0.0, 0.0), min.y),
            _ => (Vec2::new(0.04, 0.0), (min.y + max.y) * 0.5 - size * s * 0.65),
        };
        node.left = px(min.x + (max.x - min.x) * inset.x);
        node.width = px((max.x - min.x) * (1.0 - 2.0 * inset.x));
        node.top = px(top);
    }
}

/// Banner title strip in a mission: the first visible objective with work left.
fn mission_title(mv: &dishonored_mission::runtime::MissionView) -> String {
    use dishonored_mission::TaskState;
    mv.objectives
        .iter()
        .find(|o| !o.hidden && o.tasks.iter().any(|t| !t.hidden && t.state == TaskState::Active && !t.optional))
        .or_else(|| mv.objectives.iter().find(|o| !o.hidden))
        .map(|o| o.description.to_uppercase())
        .unwrap_or_else(|| mv.title.to_uppercase())
}

/// Banner name strip: the current required task.
fn mission_task(mv: &dishonored_mission::runtime::MissionView) -> String {
    use dishonored_mission::TaskState;
    if let Some(o) = &mv.outcome {
        return o.text.clone();
    }
    mv.objectives
        .iter()
        .filter(|o| !o.hidden)
        .flat_map(|o| o.tasks.iter())
        .find(|t| !t.hidden && t.state == TaskState::Active && !t.optional)
        .map(|t| t.description.clone())
        .unwrap_or_default()
}

/// Target paper: a fresh notification for a few seconds, otherwise the task list.
fn mission_paper(mv: &dishonored_mission::runtime::MissionView, now: f32, seen: &mut HashMap<u32, f32>) -> String {
    use dishonored_mission::TaskState;
    if let Some((id, title, text)) = &mv.notification {
        let t0 = *seen.entry(*id).or_insert(now);
        if now - t0 < 8.0 {
            return format!("{}\n{}", title.to_uppercase(), text);
        }
    }
    let mut s = String::new();
    for o in mv.objectives.iter().filter(|o| !o.hidden) {
        for t in o.tasks.iter().filter(|t| !t.hidden && t.state != TaskState::Inactive) {
            let mark = match t.state {
                TaskState::Completed => "  (done)",
                TaskState::Failed => "  (failed)",
                _ => "",
            };
            s += &format!("{}{mark}\n", t.description);
        }
    }
    s
}

/// Newest HUD message, for four seconds after it first appears.
fn mission_message(mv: &dishonored_mission::runtime::MissionView, now: f32, seen: &mut HashMap<u32, f32>) -> String {
    if mv.outcome.is_some() {
        return String::new();
    }
    match mv.messages.last() {
        Some((id, text)) => {
            let t0 = *seen.entry(*id).or_insert(now);
            if now - t0 < 4.0 { text.clone() } else { String::new() }
        }
        None => String::new(),
    }
}

/// End-of-mission summary in the shape of Dishonored's mission statistics.
fn mission_outcome(mv: &dishonored_mission::runtime::MissionView) -> String {
    let Some(o) = &mv.outcome else { return String::new() };
    let st = &mv.stats;
    let mins = (st.time / 60.0).floor() as i32;
    let secs = (st.time % 60.0) as i32;
    let mut s = format!("{}\n{}\n\n", if o.success { "MISSION COMPLETE" } else { "MISSION FAILED" }, o.text);
    s += &format!("Hostiles killed  {}      Civilians killed  {}\n", st.hostiles_killed, st.civilians_killed);
    s += &format!("Detected  {}      Alarms  {}      Time  {mins}:{secs:02}\n", st.detected, st.alarms);
    s += &format!("Chaos  {}", if mv.high_chaos { "High" } else { "Low" });
    let mut feats = Vec::new();
    if st.ghost() {
        feats.push("Ghost");
    }
    if st.clean_hands() {
        feats.push("Clean Hands");
    }
    if !feats.is_empty() {
        s += &format!("      {}", feats.join(", "));
    }
    s
}
