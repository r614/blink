//! Keyboard/mouse -> `PlayerInput`. Button presses between fixed ticks are latched so a
//! short click is never lost; look is applied immediately for responsiveness.
//!
//! Bindings follow the original PC layout (`DefaultInput.ini` m_PCBindings): WASD move,
//! mouse look, Space jump, C sneak (toggle), Shift sprint, LMB sword, RMB left hand (hold to
//! aim Blink), Left Ctrl block, F use, Q/E lean, R reload, X walk (no original binding),
//! 1 Blink, 2 Windblast, 3 Dark Vision, 4 Pistol, 5 Crossbow.

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
}

impl LocalInput {
    pub fn take_tick_input(&mut self) -> PlayerInput {
        if let Some(o) = self.override_input {
            self.yaw = o.yaw;
            self.pitch = o.pitch;
            self.left_hand = o.left_hand;
            return o;
        }
        let i = PlayerInput { tick: 0, forward: self.forward, right: self.right, yaw: self.yaw, pitch: self.pitch, buttons: self.held | self.latched, left_hand: self.left_hand };
        self.latched = 0;
        i
    }
}

pub fn gather_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    mut local: ResMut<LocalInput>,
) {
    if !local.enabled || local.override_input.is_some() {
        return;
    }
    let locked = cursor.iter().any(|c| c.grab_mode != CursorGrabMode::None);
    if locked {
        local.yaw = (local.yaw + motion.delta.x * 0.1).rem_euclid(360.0);
        local.pitch = (local.pitch - motion.delta.y * 0.1).clamp(-89.0, 89.0);
    }
    let axis = |pos: KeyCode, neg: KeyCode| -> i8 { (keys.pressed(pos) as i8 - keys.pressed(neg) as i8) * 127 };
    local.forward = axis(KeyCode::KeyW, KeyCode::KeyS);
    local.right = axis(KeyCode::KeyD, KeyCode::KeyA);
    let mut held = 0u16;
    let map = [
        (keys.pressed(KeyCode::Space), buttons::JUMP),
        (keys.pressed(KeyCode::KeyC), buttons::CROUCH),
        (keys.pressed(KeyCode::ShiftLeft), buttons::SPRINT),
        (keys.pressed(KeyCode::KeyX), buttons::WALK),
        (locked && mouse.pressed(MouseButton::Left), buttons::PRIMARY),
        (locked && mouse.pressed(MouseButton::Right), buttons::SECONDARY),
        (keys.pressed(KeyCode::ControlLeft), buttons::BLOCK),
        (keys.pressed(KeyCode::KeyF), buttons::USE),
        (keys.pressed(KeyCode::KeyQ), buttons::LEAN_LEFT),
        (keys.pressed(KeyCode::KeyE), buttons::LEAN_RIGHT),
        (keys.pressed(KeyCode::KeyR), buttons::RELOAD),
    ];
    for (on, b) in map {
        if on {
            held |= b;
        }
    }
    local.latched |= held & !local.held;
    local.held = held;
    if local.choice_open {
        for (k, b) in [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4].into_iter().zip(buttons::CHOICES) {
            if keys.just_pressed(k) {
                local.latched |= b;
            }
        }
        return;
    }
    for (k, h) in [
        (KeyCode::Digit1, LeftHand::Blink),
        (KeyCode::Digit2, LeftHand::Windblast),
        (KeyCode::Digit3, LeftHand::DarkVision),
        (KeyCode::Digit4, LeftHand::Pistol),
        (KeyCode::Digit5, LeftHand::Crossbow),
    ] {
        if keys.just_pressed(k) {
            local.left_hand = h;
        }
    }
}

pub fn cursor_lock(mouse: Res<ButtonInput<MouseButton>>, keys: Res<ButtonInput<KeyCode>>, mut q: Query<&mut CursorOptions, With<PrimaryWindow>>) {
    for mut c in &mut q {
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
