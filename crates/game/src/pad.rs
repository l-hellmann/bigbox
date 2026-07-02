//! Gamepad input via **bevy_gilrs** (in `DefaultPlugins`). Twin-stick mapping:
//! left stick = move, right stick = aim, right trigger = fire, bumpers = cycle
//! weapon. `read_pad` resolves one `PadInput` per frame from the first connected
//! `Gamepad`; `player_input` merges it with keyboard/mouse in `collect_commands`.
//!
//! Bevy's `Gamepad` component does edge detection for us (`just_pressed`), so
//! the old manual `gilrs` event pump + previous-frame bumper latch are gone.

use bevy::prelude::*;

/// One frame of gamepad intent, resolved from the active pad (all-`None`/`false`
/// when none is connected). Twin-stick mapping: left stick = move, right stick =
/// aim, right trigger = fire.
#[derive(Default, Clone, Copy)]
pub struct PadInput {
    /// Left-stick move vector in tile space (analog magnitude preserved),
    /// `None` inside the deadzone.
    pub move_dir: Option<(f32, f32)>,
    /// Right-stick aim direction (normalized), `None` inside the deadzone.
    pub aim_dir: Option<(f32, f32)>,
    /// Right trigger held.
    pub fire: bool,
    /// Right bumper pressed *this frame* (edge-triggered) — cycle to the next
    /// weapon.
    pub cycle_next: bool,
    /// Left bumper pressed this frame (edge) — cycle to the previous weapon.
    pub cycle_prev: bool,
}

/// Resolve the first connected gamepad into a `PadInput`. `deadzone` (0..1) is
/// the stick magnitude below which input reads as neutral.
pub fn read_pad(gamepads: &Query<&Gamepad>, deadzone: f32) -> PadInput {
    let Some(gp) = gamepads.iter().next() else {
        return PadInput::default();
    };

    // Stick Y is +up; our world dy has up = −dy (matching `W → −dy`), so flip Y.
    let deadzoned = |v: Vec2| -> Option<(f32, f32)> {
        if v.length() < deadzone {
            None
        } else {
            Some((v.x, -v.y))
        }
    };

    let move_dir = deadzoned(gp.left_stick());
    let aim_dir = deadzoned(gp.right_stick()).map(|(x, y)| {
        let len = (x * x + y * y).sqrt();
        (x / len, y / len)
    });
    let fire = gp.pressed(GamepadButton::RightTrigger2);
    // Bumpers (LB/RB) cycle weapons; `just_pressed` gives the edge natively.
    let cycle_next = gp.just_pressed(GamepadButton::RightTrigger);
    let cycle_prev = gp.just_pressed(GamepadButton::LeftTrigger);

    PadInput {
        move_dir,
        aim_dir,
        fire,
        cycle_next,
        cycle_prev,
    }
}

/// Diagnostic snapshot of one connected gamepad, for the debug overlay's
/// controller panel. Kept as a type so the (still-macroquad) debug panel keeps
/// compiling; its bevy_gilrs-based *construction* is reworked in Phase 5 — the
/// raw-`gilrs` introspection (SDL GUID / mapping / pre-mapping axis codes) it
/// used to carry has no direct `bevy_gilrs` equivalent.
#[cfg(feature = "debug")]
#[allow(dead_code)]
pub struct PadInfo {
    pub name: String,
    pub uuid: String,
    pub mapping: String,
    pub power: String,
    pub left_stick: (f32, f32),
    pub right_stick: (f32, f32),
    pub right_trigger: f32,
    pub buttons_down: Vec<&'static str>,
    pub raw_axes: Vec<(String, f32)>,
    pub raw_buttons: Vec<String>,
}

/// Gamepad status + the list of connected pads, for the debug overlay. See
/// [`PadInfo`] — reconstruction lands in Phase 5.
#[cfg(feature = "debug")]
#[allow(dead_code)]
pub struct PadDiag {
    pub initialized: bool,
    pub pads: Vec<PadInfo>,
}
