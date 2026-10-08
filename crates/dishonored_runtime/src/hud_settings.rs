//! HUD options from the settings applied on top of the HUD movie (runs after
//! `gfx_hud::update_gfx_hud`, only ever hides or fades what it showed):
//!
//! - `PSI_HUD_Visibility` (health/mana gauges): Off hides `masterHUD_mc`; Contextual shows it
//!   for 4 s after health or mana changed or while either is below full (APPROXIMATION of the
//!   original's contextual fade); Always is the movie's default.
//! - `PSI_HUD_CrosshairStyle`: Off hides every crosshair; Simple keeps only the dot.
//! - `PSI_HUD_CrosshairOpacity`: crosshair alpha.
//! - `PSI_HUD_bShowPlayerStance`: the crouch icon.
//! - `PSI_HUD_bShowObjectiveMarkers`: the projected mission marker.
//! - `PSI_HUD_bShowObjectivePopups`: applied in `hud_popups` (no automatic objectives banner or
//!   target paper; the journal key still opens them).
//! - `PSI_HUD_bShowInteractions`: the "F  Talk" prompt.

use crate::gfx_hud::{AnchoredText, HudQuad, TextKind};
use crate::session::{Session, SessionView};
use crate::settings::GameSettings;
use bevy::prelude::*;

#[derive(Default)]
pub struct Contextual {
    last: Option<(f32, f32)>,
    shown_until: f32,
}

pub fn apply(
    settings: Option<Res<GameSettings>>,
    view: Res<SessionView>,
    session: Res<Session>,
    time: Res<Time>,
    mut ctx: Local<Contextual>,
    mut quads: Query<(&HudQuad, &mut Visibility, &mut ImageNode), Without<AnchoredText>>,
    mut texts: Query<(&AnchoredText, &mut Visibility), Without<HudQuad>>,
) {
    let Some(gs) = settings else { return };
    let h = &gs.s.hud;
    let now = time.elapsed_secs();
    let vit = &session.content.sim.gameplay.vitals;
    let vitals = view.me.as_ref().map(|m| (m.vitals.health, m.vitals.mana));
    if let Some(v) = vitals {
        let below = v.0 < vit.health_max - 0.5 || v.1 < vit.mana_max - 0.5;
        if ctx.last != Some(v) || below {
            ctx.shown_until = now + 4.0;
        }
        ctx.last = Some(v);
    }
    let gauges_on = match h.visibility {
        0 => false,
        1 => now < ctx.shown_until,
        _ => true,
    };
    let cross_alpha = (h.crosshair_opacity / 100.0).clamp(0.0, 1.0);
    for (q, mut vis, mut img) in &mut quads {
        let hide = match q.group_name() {
            "gauge" | "static" if q.in_gauges() => !gauges_on,
            "cross_dot" => h.crosshair_style == 0,
            "cross" => h.crosshair_style < 2,
            "stance" => !h.player_stance,
            "marker" => !h.objective_markers,
            _ => false,
        };
        if hide {
            *vis = Visibility::Hidden;
        }
        if matches!(q.group_name(), "cross_dot" | "cross") {
            img.color.set_alpha(cross_alpha);
        }
    }
    for (t, mut vis) in &mut texts {
        let hide = match t.kind {
            TextKind::Prompt => !h.interactions,
            _ => false,
        };
        if hide {
            *vis = Visibility::Hidden;
        }
    }
}
