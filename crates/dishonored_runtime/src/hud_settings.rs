//! HUD options from the settings applied on top of the HUD movie (they only ever hide or fade
//! what the movie shows):
//!
//! - `PSI_HUD_Visibility` (health/mana gauges): Off hides `masterHUD_mc`; Contextual shows it
//!   for 4 s after health or mana changed or while either is below full (APPROXIMATION of the
//!   original's contextual fade); Always is the movie's default.
//! - `PSI_HUD_CrosshairStyle`: Off hides every crosshair; Simple keeps only the dot.
//! - `PSI_HUD_CrosshairOpacity`: crosshair alpha.
//! - `PSI_HUD_bShowPlayerStance`: the crouch icon.
//! - `PSI_HUD_bShowObjectiveMarkers`: the projected mission marker.
//! - `PSI_HUD_bShowObjectivePopups`: the mission notification paper and objective banner text.
//! - `PSI_HUD_bShowInteractions`: the "F  Talk" prompt.
//!
//! [`update`] decides them once per frame (before `gfx_hud` lays the HUD out) and `gfx_hud`
//! folds them into the values it writes, so a HUD element is only written when its final
//! value changes (a write per frame re-ran the UI layout every frame).

use crate::gfx_hud::{HudQuad, TextKind};
use crate::session::{Session, SessionView};
use crate::settings::GameSettings;
use bevy::prelude::*;

#[derive(Default)]
pub struct Contextual {
    last: Option<(f32, f32)>,
    shown_until: f32,
}

/// This frame's HUD options (None before the settings exist: the movie's defaults).
#[derive(Resource, Default)]
pub struct HudOverrides(Option<Active>);

struct Active {
    hud: dishonored_settings::Hud,
    gauges_on: bool,
}

impl HudOverrides {
    /// Whether the options hide a quad, and the crosshair alpha they give it.
    pub fn quad(&self, q: &HudQuad) -> (bool, Option<f32>) {
        let Some(a) = &self.0 else { return (false, None) };
        let h = &a.hud;
        let hide = match q.group_name() {
            "gauge" | "static" if q.in_gauges() => !a.gauges_on,
            "cross_dot" => h.crosshair_style == 0,
            "cross" => h.crosshair_style < 2,
            "stance" => !h.player_stance,
            "marker" => !h.objective_markers,
            "target_note" => !h.objective_popups,
            _ => false,
        };
        let alpha = matches!(q.group_name(), "cross_dot" | "cross").then(|| (h.crosshair_opacity / 100.0).clamp(0.0, 1.0));
        (hide, alpha)
    }

    /// Whether the options hide an anchored text.
    pub fn text_hidden(&self, kind: TextKind) -> bool {
        let Some(a) = &self.0 else { return false };
        match kind {
            TextKind::Prompt => !a.hud.interactions,
            TextKind::TargetBody => !a.hud.objective_popups,
            _ => false,
        }
    }
}

pub fn update(settings: Option<Res<GameSettings>>, view: Res<SessionView>, session: Res<Session>, time: Res<Time>, mut ctx: Local<Contextual>, mut out: ResMut<HudOverrides>) {
    let Some(gs) = settings else {
        out.0 = None;
        return;
    };
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
    out.0 = Some(Active { hud: h.clone(), gauges_on });
}
