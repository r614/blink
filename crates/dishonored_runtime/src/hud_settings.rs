//! The HUD options (`DisGFxMoviePlayerHUD.m_Settings`, a `DisHUDSettings` copied from the
//! profile's `PSI_HUD_*` values) and where the native HUD applies them. `gfx_hud.rs` reads them
//! every frame from [`options`]:
//!
//! | Option | Original use | Port |
//! |---|---|---|
//! | `PSI_HUD_Visibility` (Off / Contextual / Always) | player status shown always, never, or for `m_fPlayerInfoDisplayDuration` (8 s) after a gauge value changes (0x009EA160 restarts the timer) and while low on health / mana (`m_bAlwaysShowPlayerInfoWhenLow*`) | applied |
//! | `PSI_HUD_bShowObjectivePopups` | the popup queue is emptied instead of shown (0x00BAFE70) | applied |
//! | `PSI_HUD_bShowInteractions` | interaction texts | applied (interactions window) |
//! | `PSI_HUD_bShowPickupLog` | `AddLogItem` does nothing (0x00B98630) | applied |
//! | `PSI_HUD_bShowPlayerStance` | stance icon (0x00B95ED0) | applied |
//! | `PSI_HUD_CrosshairStyle` (Off / Simple / Normal) | no crosshair / dot only / item crosshairs (0x00B9FF30) | applied |
//! | `PSI_HUD_bCrosshairMovement` | the crosshair moves to the focused interactable | applied |
//! | `PSI_HUD_CrosshairOpacity` | crosshair alpha (dot and aspect) | applied |
//! | `PSI_HUD_bShowObjectiveMarkers`, `...AwarenessMarkers`, `...GrenadeMarkers`, `...HeartTargetMarkers` | the marker lists | markers not drawn yet |
//! | `PSI_HUD_bShowTutorialNotifications`, `...Highlight`, `...ContextualIcons` | tutorials, object highlight, context icons | not drawn yet |

use crate::settings::GameSettings;

/// The HUD options in effect (the defaults before the settings exist).
pub fn options(settings: Option<&GameSettings>) -> dishonored_settings::Hud {
    settings.map(|s| s.s.hud.clone()).unwrap_or_default()
}
