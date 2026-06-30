//! Gamepad polling. Twin-stick mapping: left stick = move, right stick = aim,
//! right trigger = fire. Resolves one `PadInput` per frame; the runtime merges
//! it with keyboard/mouse in `collect_input`.
//!
//! Native uses `gilrs`. On wasm the web build ships a no-op stub: gilrs reaches
//! the browser Gamepad API through wasm-bindgen, whose JS glue macroquad's plain
//! loader can't provide — so gamepad-on-web is deferred (keyboard/mouse work).

#[cfg(not(target_arch = "wasm32"))]
use gilrs::{Axis, Button, Gilrs};

/// One frame of gamepad intent, resolved from the active pad (all-`None` when
/// none is connected, or on wasm). Twin-stick mapping: left stick = move,
/// right stick = aim, right trigger = fire.
#[derive(Default, Clone, Copy)]
pub struct PadInput {
    /// Left-stick move vector in tile space (analog magnitude preserved),
    /// `None` inside the deadzone.
    pub move_dir: Option<(f32, f32)>,
    /// Right-stick aim direction (normalized), `None` inside the deadzone.
    pub aim_dir: Option<(f32, f32)>,
    /// Right trigger held.
    pub fire: bool,
}

/// Diagnostic snapshot of one connected gamepad, for the debug overlay's
/// controller panel.
#[cfg(feature = "debug")]
pub struct PadInfo {
    pub name: String,
    /// SDL GUID (hex) — the key a custom mapping is written against.
    pub uuid: String,
    /// SDL mapping name, or a flag that the device is unmapped (axes/buttons
    /// won't resolve without a mapping — the usual "connected but dead" cause).
    pub mapping: String,
    pub power: String,
    pub left_stick: (f32, f32),
    pub right_stick: (f32, f32),
    pub right_trigger: f32,
    pub buttons_down: Vec<&'static str>,
    /// Raw (pre-mapping) axis code → value, for identifying which physical
    /// stick/trigger maps to which native code on an unmapped pad.
    pub raw_axes: Vec<(String, f32)>,
    /// Raw button codes currently pressed.
    pub raw_buttons: Vec<String>,
}

/// gilrs status + the list of connected pads, for the debug overlay.
#[cfg(feature = "debug")]
pub struct PadDiag {
    pub initialized: bool,
    pub pads: Vec<PadInfo>,
}

/// Native gamepad polling via `gilrs`.
#[cfg(not(target_arch = "wasm32"))]
pub struct Pads(Option<Gilrs>);

#[cfg(not(target_arch = "wasm32"))]
impl Pads {
    pub fn new() -> Self {
        Pads(Gilrs::new().ok())
    }

    /// Poll the first connected gamepad for this frame's intent. `deadzone`
    /// (0..1) is the stick magnitude below which input reads as neutral.
    pub fn read(&mut self, deadzone: f32) -> PadInput {
        let Some(gilrs) = self.0.as_mut() else {
            return PadInput::default();
        };
        // Draining events refreshes the cached pad state as a side effect.
        while gilrs.next_event().is_some() {}
        let Some((_id, gp)) = gilrs.gamepads().next() else {
            return PadInput::default();
        };

        // Stick Y is +up; our world dy has up = −dy (matching `W → −dy`),
        // so flip Y for both vectors.
        let deadzoned = |x: f32, y: f32| -> Option<(f32, f32)> {
            if (x * x + y * y).sqrt() < deadzone {
                None
            } else {
                Some((x, -y))
            }
        };

        let move_dir = deadzoned(gp.value(Axis::LeftStickX), gp.value(Axis::LeftStickY));
        let aim_dir = deadzoned(gp.value(Axis::RightStickX), gp.value(Axis::RightStickY)).map(
            |(x, y)| {
                let len = (x * x + y * y).sqrt();
                (x / len, y / len)
            },
        );
        let fire = gp.is_pressed(Button::RightTrigger2);

        PadInput {
            move_dir,
            aim_dir,
            fire,
        }
    }

    /// Diagnostic snapshot of every connected pad (debug overlay only).
    #[cfg(feature = "debug")]
    pub fn debug_diag(&self) -> PadDiag {
        const BUTTONS: &[(&str, Button)] = &[
            ("A", Button::South),
            ("B", Button::East),
            ("X", Button::West),
            ("Y", Button::North),
            ("LB", Button::LeftTrigger),
            ("RB", Button::RightTrigger),
            ("LT", Button::LeftTrigger2),
            ("RT", Button::RightTrigger2),
            ("Start", Button::Start),
            ("Select", Button::Select),
            ("L3", Button::LeftThumb),
            ("R3", Button::RightThumb),
            ("Up", Button::DPadUp),
            ("Down", Button::DPadDown),
            ("Left", Button::DPadLeft),
            ("Right", Button::DPadRight),
        ];
        let Some(gilrs) = self.0.as_ref() else {
            return PadDiag {
                initialized: false,
                pads: Vec::new(),
            };
        };
        let pads = gilrs
            .gamepads()
            .map(|(_, gp)| {
                let st = gp.state();
                let raw_axes = st
                    .axes()
                    .map(|(code, data)| (format!("{code}"), data.value()))
                    .collect();
                let raw_buttons = st
                    .buttons()
                    .filter(|(_, d)| d.is_pressed())
                    .map(|(code, _)| format!("{code}"))
                    .collect();
                PadInfo {
                    name: gp.name().to_string(),
                    uuid: gp.uuid().iter().map(|b| format!("{b:02x}")).collect(),
                    mapping: gp
                        .map_name()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "UNMAPPED — no SDL mapping".to_string()),
                    power: format!("{:?}", gp.power_info()),
                    left_stick: (gp.value(Axis::LeftStickX), gp.value(Axis::LeftStickY)),
                    right_stick: (gp.value(Axis::RightStickX), gp.value(Axis::RightStickY)),
                    right_trigger: gp
                        .button_data(Button::RightTrigger2)
                        .map(|d| d.value())
                        .unwrap_or(0.0),
                    buttons_down: BUTTONS
                        .iter()
                        .filter(|(_, b)| gp.is_pressed(*b))
                        .map(|(n, _)| *n)
                        .collect(),
                    raw_axes,
                    raw_buttons,
                }
            })
            .collect();
        PadDiag {
            initialized: true,
            pads,
        }
    }
}

/// Web stub: no gamepad backend (see the module note). Keyboard/mouse input is
/// unaffected — it flows through macroquad, not here.
#[cfg(target_arch = "wasm32")]
pub struct Pads;

#[cfg(target_arch = "wasm32")]
impl Pads {
    pub fn new() -> Self {
        Pads
    }

    /// Always neutral: no pad on web.
    pub fn read(&mut self, _deadzone: f32) -> PadInput {
        PadInput::default()
    }

    /// Reports "no backend" to the debug overlay (debug builds don't target
    /// wasm in practice, but keep the surface consistent).
    #[cfg(feature = "debug")]
    pub fn debug_diag(&self) -> PadDiag {
        PadDiag {
            initialized: false,
            pads: Vec::new(),
        }
    }
}
