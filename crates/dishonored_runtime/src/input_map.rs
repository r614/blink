//! Keyboard/mouse -> `PlayerInput`. Button presses between fixed ticks are latched so a
//! short click is never lost; look is applied immediately for responsiveness.
//!
//! Bindings follow the original PC layout (`DefaultInput.ini` m_PCBindings): WASD move,
//! mouse look, Space jump, C sneak (toggle), Shift sprint, LMB sword, RMB left hand (hold to
//! aim Blink), Left Ctrl block, F use, Q/E lean, R reload, X walk (no original binding),
//! 1 Blink, 2 Windblast, 3 Dark Vision, 4 Pistol, 5 Crossbow.
//! The keys come from the player's settings (`settings::Bindings`, rebindable in Options ->
//! Controls -> Keyboard Mapping); the list above is the default.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};

#[derive(Resource, Default)]
pub struct LocalInput {
    pub yaw: f32,
    pub pitch: f32,
    pub forward: i8,
    pub right: i8,
    pub held: u16,
    pub latched: u16,
    pub left_hand: LeftHand,
    /// When set (harness script / control socket), replaces keyboard input.
    pub override_input: Option<PlayerInput>,
    pub enabled: bool,
    /// A dialog choice is open for the local player: number keys pick options instead of powers.
    pub choice_open: bool,
    /// One-shot item action (`dishonored_sim::input::actions`) latched until the next tick.
    pub action: u8,
}

impl LocalInput {
    pub fn take_tick_input(&mut self) -> PlayerInput {
        if let Some(o) = self.override_input {
            self.yaw = o.yaw;
            self.pitch = o.pitch;
            self.left_hand = o.left_hand;
            return o;
        }
        let i = PlayerInput { tick: 0, forward: self.forward, right: self.right, yaw: self.yaw, pitch: self.pitch, buttons: self.held | self.latched, left_hand: self.left_hand, action: self.action };
        self.latched = 0;
        self.action = 0;
        i
    }
}

/// Mouse smoothing (`PSI_Mouse_bSmooting`): average of this and the previous frame's motion,
/// like UE3's `bEnableMouseSmoothing` (APPROXIMATION: the engine's sample-time weighting is
/// not reproduced).
#[derive(Default)]
pub struct Smoothing(Vec2);

#[allow(clippy::too_many_arguments)]
pub fn gather_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<bevy::input::mouse::AccumulatedMouseScroll>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    settings: Option<Res<crate::settings::GameSettings>>,
    bindings: Option<Res<crate::settings::Bindings>>,
    mut smooth: Local<Smoothing>,
    mut local: ResMut<LocalInput>,
) {
    if !local.enabled || local.override_input.is_some() {
        return;
    }
    let default_bindings;
    let bind = match &bindings {
        Some(b) => b.as_ref(),
        None => {
            default_bindings = crate::settings::Bindings::from_settings(&dishonored_settings::Settings::default());
            &default_bindings
        }
    };
    let (scale, invert, smoothing) = settings.as_ref().map(|s| (s.s.mouse_degrees_per_count(), s.s.mouse.invert_y, s.s.mouse.smoothing)).unwrap_or((0.1, false, false));
    let locked = cursor.iter().any(|c| c.grab_mode != CursorGrabMode::None);
    if locked {
        let raw = motion.delta;
        let d = if smoothing { (raw + smooth.0) * 0.5 } else { raw };
        smooth.0 = raw;
        let dy = if invert { -d.y } else { d.y };
        local.yaw = (local.yaw + d.x * scale).rem_euclid(360.0);
        local.pitch = (local.pitch - dy * scale).clamp(-89.0, 89.0);
    }
    let wheel = scroll.delta.y;
    let on = |action: &str| bind.pressed(action, &keys, &mouse, wheel) && (locked || !is_mouse(bind, action));
    let axis = |pos: &str, neg: &str| -> i8 { (on(pos) as i8 - on(neg) as i8) * 127 };
    local.forward = axis("GBA_MoveForward", "GBA_MoveBackward");
    local.right = axis("GBA_StrafeRight", "GBA_StrafeLeft");
    let mut held = 0u16;
    let map = [
        ("GBA_Jump", buttons::JUMP),
        ("GBA_Sneak", buttons::CROUCH),
        ("GBA_Sprint", buttons::SPRINT),
        ("PORT_Walk", buttons::WALK),
        ("GBA_Primary", buttons::PRIMARY),
        ("GBA_Secondary", buttons::SECONDARY),
        ("GBA_Block", buttons::BLOCK),
        ("GBA_Use", buttons::USE),
        ("GBA_LeanLeft", buttons::LEAN_LEFT),
        ("GBA_LeanRight", buttons::LEAN_RIGHT),
        ("PORT_Reload", buttons::RELOAD),
    ];
    for (action, b) in map {
        if on(action) {
            held |= b;
        }
    }
    local.latched |= held & !local.held;
    local.held = held;
    // `Dis_ConsumeElixir` (GBA_HealthElixir / GBA_ManaElixir).
    if bind.just_pressed("GBA_HealthElixir", &keys, &mouse, wheel) {
        local.action = dishonored_sim::input::actions::DRINK_HEALTH;
    } else if bind.just_pressed("GBA_ManaElixir", &keys, &mouse, wheel) {
        local.action = dishonored_sim::input::actions::DRINK_MANA;
    }
    if local.choice_open {
        for (k, b) in [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4].into_iter().zip(buttons::CHOICES) {
            if keys.just_pressed(k) {
                local.latched |= b;
            }
        }
        return;
    }
    for (action, h) in [
        ("GBA_Shortcut_0", LeftHand::Blink),
        ("GBA_Shortcut_1", LeftHand::Windblast),
        ("GBA_Shortcut_2", LeftHand::DarkVision),
        ("GBA_Shortcut_3", LeftHand::Pistol),
        ("GBA_Shortcut_4", LeftHand::Crossbow),
        ("GBA_Shortcut_5", LeftHand::Possession),
        ("GBA_Shortcut_6", LeftHand::BendTime),
        ("GBA_Shortcut_7", LeftHand::DevouringSwarm),
        ("GBA_Shortcut_8", LeftHand::Grenade),
        ("GBA_Shortcut_9", LeftHand::SpringRazor),
    ] {
        if bind.just_pressed(action, &keys, &mouse, wheel) {
            local.left_hand = h;
        }
    }
}

fn is_mouse(b: &crate::settings::Bindings, action: &str) -> bool {
    matches!(b.input(action), Some(crate::settings::Input::Mouse(_)))
}

/// Click captures the mouse; Escape releases it (and opens the pause menu, `pause_menu.rs`).
/// While the pause menu is open the cursor stays free.
pub fn cursor_lock(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    paused: Option<Res<crate::pause_menu::PauseMenu>>,
    mut q: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let open = paused.is_some_and(|p| p.open);
    for mut c in &mut q {
        if open {
            if c.grab_mode != CursorGrabMode::None {
                c.grab_mode = CursorGrabMode::None;
                c.visible = true;
            }
            continue;
        }
        if mouse.just_pressed(MouseButton::Left) && c.grab_mode == CursorGrabMode::None {
            c.grab_mode = CursorGrabMode::Locked;
            c.visible = false;
        }
        if keys.just_pressed(KeyCode::Escape) {
            c.grab_mode = CursorGrabMode::None;
            c.visible = true;
        }
    }
}
