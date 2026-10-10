//! Bodies burnt to ash (Shadow Kill, walls of light; `dishonored_assets::shadow_kill`): after the
//! delay the body fades to black (`Sk_Fade_To_Black` from 1 to 0, taken as scaling the body's
//! colour), then wears the fiery material — black, kept where its mask clears the threshold
//! `F_Firy_Amount` sets — while the amount runs from 1 to 0, and then it is gone with what it
//! holds. A body removed without ash (gibbed) or first seen already removed is gone at once.
//! Presentation only: the sim removed the body (`PlayerState::no_body`) at the kill.

use crate::char_light::CharMaterial;
use crate::characters::CharacterRoot;
use crate::players::PlayerBody;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use dishonored_assets::shadow_kill::{AshBodyDef, fiery_cutoff};
use std::collections::HashMap;

/// A character mesh of the body itself (a skeletal part, not an accessory or a held item): what the
/// sequence's materials apply to.
#[derive(Component)]
pub struct BodyPartMesh;

#[derive(Resource, Default)]
pub struct AshBodies {
    /// Bodies the sim removed (`SimEvent::BodyRemoved`): (burnt to ash, presentation time).
    pub removed: HashMap<u32, (bool, f64)>,
    /// The sequence and the fiery material's mask texture, loaded on first use.
    def: Option<Option<(AshBodyDef, Handle<Image>)>>,
    burns: HashMap<u32, Burn>,
}

#[derive(Default)]
struct Burn {
    /// Fade to black: each body mesh's material and its colour before the fade.
    faded: Vec<(Entity, Mat, LinearRgba)>,
    fiery: Option<Handle<StandardMaterial>>,
}

enum Mat {
    Char(Handle<CharMaterial>),
    Std(Handle<StandardMaterial>),
}

/// The fiery material's mask (`F_Firy_Mask`) as an alpha texture (alpha = the red channel as
/// sampled, linear), repeating.
fn load_mask(def: &AshBodyDef, images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let content = dishonored_assets::Content::locate().ok()?;
    let pkg = content.package(&def.mask.0).ok()?;
    let e = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "Texture2D" && pkg.obj_path(ue3_package::ObjRef::Export(i)).eq_ignore_ascii_case(&def.mask.1))?;
    let t = dishonored_assets::texture::read_texture(&content, &pkg, e, 512).ok()?;
    let (w, h, px) = dishonored_assets::texture::decode_rgba8(&t, 0)?;
    let lin = |v: u8| {
        let c = v as f32 / 255.0;
        if t.srgb { if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) } } else { c }
    };
    let data: Vec<u8> = px.iter().flat_map(|p| [255, 255, 255, (lin(p[0]) * 255.0).round() as u8]).collect();
    let mut img = Image::new(Extent3d { width: w, height: h, depth_or_array_layers: 1 }, TextureDimension::D2, data, TextureFormat::Rgba8Unorm, bevy::asset::RenderAssetUsages::RENDER_WORLD);
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor { address_mode_u: ImageAddressMode::Repeat, address_mode_v: ImageAddressMode::Repeat, ..ImageSamplerDescriptor::linear() });
    Some(images.add(img))
}

/// Run the sequence of each removed body and hide the ones it finished (after the poses and the
/// players' visibility of this frame).
#[allow(clippy::too_many_arguments)]
pub fn update(
    mut commands: Commands,
    mut ash: ResMut<AshBodies>,
    view: Res<crate::session::SessionView>,
    time: Res<Time>,
    mut images: ResMut<Assets<Image>>,
    mut std_mats: ResMut<Assets<StandardMaterial>>,
    mut char_mats: ResMut<Assets<CharMaterial>>,
    mut roots: Query<(Entity, &PlayerBody, &Children, &mut Visibility), With<CharacterRoot>>,
    parts: Query<(Option<&MeshMaterial3d<StandardMaterial>>, Option<&MeshMaterial3d<CharMaterial>>), With<BodyPartMesh>>,
) {
    let now = time.elapsed_secs_f64();
    if ash.def.is_none() {
        let def = dishonored_assets::Content::locate().ok().and_then(|c| dishonored_assets::shadow_kill::ash_body_def(&c).map_err(|e| warn!("shadow kill: {e:#}")).ok());
        ash.def = Some(def.and_then(|d| load_mask(&d, &mut images).map(|m| (d, m))));
    }
    let def = ash.def.clone().flatten();
    for (root, body, children, mut vis) in &mut roots {
        let Some(p) = view.others.iter().chain(view.me.iter()).find(|p| p.id == body.0) else { continue };
        if p.alive || !p.no_body {
            ash.removed.remove(&p.id);
            // A body that burnt and lives again (a respawn) is rebuilt with its own materials.
            if ash.burns.remove(&p.id).is_some() {
                commands.entity(root).despawn();
            }
            continue;
        }
        let (t, (d, mask)) = match (ash.removed.get(&p.id), def.as_ref()) {
            (Some((true, t0)), Some(d)) => ((now - t0) as f32, d),
            _ => {
                *vis = Visibility::Hidden;
                continue;
            }
        };
        let (fade_at, fiery_at, gone_at) = (d.delay, d.delay + d.fade_to_black, d.delay + d.fade_to_black + d.fiery);
        if t >= gone_at {
            *vis = Visibility::Hidden;
            continue;
        }
        let burn = ash.burns.entry(p.id).or_default();
        if t >= fiery_at {
            // The fiery material on the body's meshes, its threshold from the amount.
            let h = burn.fiery.get_or_insert_with(|| {
                let (s, o) = (Vec2::new(0.5 * d.tiling, 0.25 * d.tiling), Vec2::new(0.5 * d.tiling, 0.25 * d.tiling));
                let h = std_mats.add(StandardMaterial {
                    base_color: Color::BLACK,
                    base_color_texture: Some(mask.clone()),
                    unlit: true,
                    alpha_mode: AlphaMode::Mask(fiery_cutoff(1.0, d.edge_size)),
                    uv_transform: bevy::math::Affine2::from_scale_angle_translation(s, 0.0, o),
                    ..Default::default()
                });
                for c in children.iter().filter(|c| parts.contains(*c)) {
                    commands.entity(c).remove::<MeshMaterial3d<CharMaterial>>().remove::<MeshMaterial3d<StandardMaterial>>().insert(MeshMaterial3d(h.clone()));
                }
                h
            });
            let amount = 1.0 - ((t - fiery_at) / d.fiery.max(1e-3)).clamp(0.0, 1.0);
            if let Some(mut m) = std_mats.get_mut(&*h) {
                m.alpha_mode = AlphaMode::Mask(fiery_cutoff(amount, d.edge_size));
            }
        } else if t >= fade_at {
            // Fade to black: the body's own material instances (a shared one is copied first).
            if burn.faded.is_empty() {
                for c in children.iter() {
                    match parts.get(c) {
                        Ok((_, Some(cm))) => {
                            if let Some(m) = char_mats.get(&cm.0) {
                                burn.faded.push((c, Mat::Char(cm.0.clone()), m.base.base_color.to_linear()));
                            }
                        }
                        Ok((Some(sm), None)) => {
                            if let Some(m) = std_mats.get(&sm.0).cloned() {
                                let colour = m.base_color.to_linear();
                                let h = std_mats.add(m);
                                commands.entity(c).insert(MeshMaterial3d(h.clone()));
                                burn.faded.push((c, Mat::Std(h), colour));
                            }
                        }
                        _ => {}
                    }
                }
            }
            let k = 1.0 - ((t - fade_at) / d.fade_to_black.max(1e-3)).clamp(0.0, 1.0);
            for (_, m, c) in &burn.faded {
                let col = Color::linear_rgba(c.red * k, c.green * k, c.blue * k, c.alpha);
                match m {
                    Mat::Char(h) => {
                        if let Some(mut m) = char_mats.get_mut(h) {
                            m.base.base_color = col;
                        }
                    }
                    Mat::Std(h) => {
                        if let Some(mut m) = std_mats.get_mut(h) {
                            m.base_color = col;
                        }
                    }
                }
            }
        }
    }
    // Pawns gone from the view.
    let live: Vec<u32> = view.others.iter().chain(view.me.iter()).map(|p| p.id).collect();
    ash.burns.retain(|id, _| live.contains(id));
    ash.removed.retain(|id, _| live.contains(id));
}
