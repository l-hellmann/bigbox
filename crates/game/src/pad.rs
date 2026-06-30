//! Gamepad polling. Twin-stick mapping: left stick = move, right stick = aim,
//! right trigger = fire. Resolves one `PadInput` per frame; the runtime merges
//! it with keyboard/mouse in `collect_input`.
//!
//! Native uses `gilrs`. gilrs *also* has a web backend, but it reaches the
//! browser Gamepad API through wasm-bindgen, whose JS glue macroquad's plain
//! loader can't provide — so on wasm we skip gilrs and read the Gamepad API
//! ourselves via a small miniquad plugin (`web/quad-gamepad.js`), passing the
//! numeric axes/buttons straight across the boundary. Same twin-stick mapping
//! either way.

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
    /// Right bumper pressed *this frame* (edge-triggered) — cycle to the next
    /// weapon. Edge detection lives in [`Pads`] since `PadInput` is per-frame.
    pub cycle_next: bool,
    /// Left bumper pressed this frame (edge) — cycle to the previous weapon.
    pub cycle_prev: bool,
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

/// Native gamepad polling via `gilrs`. Carries the previous-frame bumper state
/// so weapon-cycle inputs can be edge-detected (gilrs has no built-in edge).
#[cfg(not(target_arch = "wasm32"))]
pub struct Pads {
    gilrs: Option<Gilrs>,
    prev_next: bool,
    prev_prev: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl Pads {
    pub fn new() -> Self {
        Pads {
            gilrs: Gilrs::new().ok(),
            prev_next: false,
            prev_prev: false,
        }
    }

    /// Poll the first connected gamepad for this frame's intent. `deadzone`
    /// (0..1) is the stick magnitude below which input reads as neutral.
    pub fn read(&mut self, deadzone: f32) -> PadInput {
        let Some(gilrs) = self.gilrs.as_mut() else {
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
        // Bumpers (LB/RB) cycle weapons; edge-detect against the previous frame.
        let rb = gp.is_pressed(Button::RightTrigger);
        let lb = gp.is_pressed(Button::LeftTrigger);
        let cycle_next = rb && !self.prev_next;
        let cycle_prev = lb && !self.prev_prev;
        self.prev_next = rb;
        self.prev_prev = lb;

        PadInput {
            move_dir,
            aim_dir,
            fire,
            cycle_next,
            cycle_prev,
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
        let Some(gilrs) = self.gilrs.as_ref() else {
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

/// Web gamepad polling via the browser Gamepad API, bridged by `quad-gamepad.js`
/// (a miniquad plugin). The API's axes/buttons are plain numbers, so they cross
/// the wasm boundary directly — no wasm-bindgen, no string marshaling, unlike
/// gilrs's web backend (the whole reason gilrs is native-only here).
#[cfg(target_arch = "wasm32")]
mod web {
    use super::PadInput;

    // Imports provided by `web/quad-gamepad.js`. All numeric.
    unsafe extern "C" {
        /// 1 if a connected gamepad is present (and cached for this poll), else 0.
        fn quad_gamepad_connected() -> i32;
        /// Axis value of the cached gamepad, or 0.0 if out of range.
        fn quad_gamepad_axis(index: i32) -> f32;
        /// Button analog value (0..1) of the cached gamepad, or 0.0.
        fn quad_gamepad_button(index: i32) -> f32;
    }

    // "Standard" gamepad mapping indices — the same physical controls the native
    // gilrs path reads (left stick = move, right stick = aim, RT = fire).
    const AXIS_LEFT_X: i32 = 0;
    const AXIS_LEFT_Y: i32 = 1;
    const AXIS_RIGHT_X: i32 = 2;
    const AXIS_RIGHT_Y: i32 = 3;
    const BUTTON_LEFT_BUMPER: i32 = 4;
    const BUTTON_RIGHT_BUMPER: i32 = 5;
    const BUTTON_RIGHT_TRIGGER: i32 = 7;
    /// Analog trigger reads as "held" past this value.
    const TRIGGER_THRESHOLD: f32 = 0.5;

    /// Carries the previous-frame bumper state so weapon-cycle inputs edge-trigger
    /// (the Gamepad API reports held state, not presses).
    #[derive(Default)]
    pub struct Pads {
        prev_next: bool,
        prev_prev: bool,
    }

    impl Pads {
        pub fn new() -> Self {
            Pads::default()
        }

        /// Poll the first connected gamepad. `deadzone` (0..1) matches the native
        /// path's stick-magnitude neutral zone.
        pub fn read(&mut self, deadzone: f32) -> PadInput {
            // `connected()` refreshes the cached gamepad snapshot as a side
            // effect, so it must be called before reading axes/buttons.
            if unsafe { quad_gamepad_connected() } == 0 {
                return PadInput::default();
            }
            let axis = |i| unsafe { quad_gamepad_axis(i) };
            let button = |i| unsafe { quad_gamepad_button(i) };

            // The Gamepad API already reports stick Y as +down, which *is* our
            // world dy (up = −dy). So, unlike the gilrs path (which gets +up and
            // flips), we pass Y straight through.
            let deadzoned = |x: f32, y: f32| -> Option<(f32, f32)> {
                if (x * x + y * y).sqrt() < deadzone {
                    None
                } else {
                    Some((x, y))
                }
            };

            let move_dir = deadzoned(axis(AXIS_LEFT_X), axis(AXIS_LEFT_Y));
            let aim_dir = deadzoned(axis(AXIS_RIGHT_X), axis(AXIS_RIGHT_Y)).map(|(x, y)| {
                let len = (x * x + y * y).sqrt();
                (x / len, y / len)
            });
            let fire = button(BUTTON_RIGHT_TRIGGER) > TRIGGER_THRESHOLD;
            // Bumpers cycle weapons; edge-detect against the previous frame.
            let rb = button(BUTTON_RIGHT_BUMPER) > TRIGGER_THRESHOLD;
            let lb = button(BUTTON_LEFT_BUMPER) > TRIGGER_THRESHOLD;
            let cycle_next = rb && !self.prev_next;
            let cycle_prev = lb && !self.prev_prev;
            self.prev_next = rb;
            self.prev_prev = lb;

            PadInput {
                move_dir,
                aim_dir,
                fire,
                cycle_next,
                cycle_prev,
            }
        }

        /// The debug overlay isn't built for wasm (egui is native-only), but keep
        /// the surface so the module compiles under any feature combination.
        #[cfg(feature = "debug")]
        pub fn debug_diag(&self) -> super::PadDiag {
            super::PadDiag {
                initialized: unsafe { quad_gamepad_connected() } != 0,
                pads: Vec::new(),
            }
        }
    }

    /// Exported so `mq_js_bundle.js`'s plugin version check matches the JS side
    /// (`version: 1`) and stays silent. Kept by `#[unsafe(no_mangle)]`.
    #[unsafe(no_mangle)]
    pub extern "C" fn quad_gamepad_crate_version() -> u32 {
        1
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::Pads;
