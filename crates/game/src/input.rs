//! Input layer: turn raw devices (keyboard, mouse, gamepad) into gameplay
//! intent. The aim-source latch lives here as `AimState` — the one piece of
//! per-frame input state that carries across frames — kept pure (it takes the
//! mouse position + ground hit as arguments rather than reading engine globals)
//! so the mouse-vs-pad switching logic is unit-testable headlessly.
//!
//! Bevy port: `AimState` is a `Resource`; the device reads (`collect_commands`)
//! take Bevy's `ButtonInput` handles + a pre-accumulated wheel delta so the
//! command-building stays a plain function the `player_input` system feeds.

use crate::pad::PadInput;
use bb_game::{Command, Player};
use bevy::prelude::*;

/// Which device currently drives aim. The right stick switches to `Pad` (and
/// the last direction is held when the stick re-centers); only actual mouse
/// movement switches back to `Mouse`. Prevents the aim snapping to the cursor
/// the moment you let go of the stick.
#[derive(Clone, Copy, PartialEq, Debug)]
enum AimSource {
    Mouse,
    Pad,
}

/// The aim resolved for one frame: the tile-space direction the player faces
/// (feeds `Command::Fire`) and the 3D marker point for the crosshair viz.
pub struct AimFrame {
    /// Normalized aim direction in tile-space, or `None` when nothing aims.
    pub dir: Option<(f32, f32)>,
    /// World-space point the crosshair marks — the mouse ground-hit, or a
    /// point ahead of the player when aiming on the stick. Consumed by the
    /// crosshair render in Phase 3; unread until then.
    #[allow(dead_code)]
    pub hit: Option<Vec3>,
}

/// The mouse-vs-pad aim latch plus the held heading. A Bevy resource; one
/// `update` per frame (in `player_input`) produces that frame's `AimFrame`.
#[derive(Resource)]
pub struct AimState {
    source: AimSource,
    /// Last pad-aim direction, held when the right stick re-centers.
    held: Option<(f32, f32)>,
    prev_mouse: (f32, f32),
}

impl AimState {
    pub fn new(mouse_pos: (f32, f32)) -> Self {
        AimState {
            source: AimSource::Mouse,
            held: None,
            prev_mouse: mouse_pos,
        }
    }

    /// Last cursor position the latch saw — used when the cursor leaves the
    /// window (`cursor_position()` is `None`) so the absence doesn't read as
    /// movement and yank the aim source back to the mouse.
    pub fn last_mouse(&self) -> (f32, f32) {
        self.prev_mouse
    }

    /// Advance the latch one frame. `mouse_pos` is this frame's cursor position
    /// and `mouse_hit` its ground-plane projection (from `ground_hit`); both are
    /// passed in rather than read from globals so this stays testable.
    pub fn update(
        &mut self,
        player: &Player,
        pad: &PadInput,
        mouse_pos: (f32, f32),
        mouse_hit: Option<Vec3>,
    ) -> AimFrame {
        // Only real mouse movement (> 1px) hands aim back to the mouse.
        let mouse_moved = (mouse_pos.0 - self.prev_mouse.0).abs() > 1.0
            || (mouse_pos.1 - self.prev_mouse.1).abs() > 1.0;
        self.prev_mouse = mouse_pos;

        // Active controller input (aiming or moving) claims the aim source over
        // the mouse; the mouse only reclaims it on real movement.
        if pad.aim_dir.is_some() || pad.move_dir.is_some() {
            self.source = AimSource::Pad;
        } else if mouse_moved {
            self.source = AimSource::Mouse;
        }

        let dir = match self.source {
            AimSource::Pad => {
                // Right stick aims; otherwise face the movement direction;
                // otherwise hold the last heading.
                let active = pad.aim_dir.or_else(|| pad.move_dir.map(normalize));
                if active.is_some() {
                    self.held = active;
                }
                active.or(self.held)
            }
            AimSource::Mouse => aim_direction(player, mouse_hit),
        };

        // Aim marker (3D crosshair): a point ahead of the player when aiming on
        // the stick, else the mouse hit.
        let hit = match self.source {
            AimSource::Pad => {
                dir.map(|(ax, ay)| Vec3::new(player.x + ax * 6.0, 0.0, player.y + ay * 6.0))
            }
            AimSource::Mouse => mouse_hit,
        };

        AimFrame { dir, hit }
    }
}

/// Normalize a 2D vector, or `(0, 0)` if it's degenerate.
fn normalize((x, y): (f32, f32)) -> (f32, f32) {
    let len = (x * x + y * y).sqrt();
    if len > 1e-6 {
        (x / len, y / len)
    } else {
        (0.0, 0.0)
    }
}

/// Ground hit point → normalized aim direction in **tile-space** relative to
/// the player (`(dx, dy)` where dy is along world Z). `None` when there's no
/// hit or the cursor sits on top of the player.
fn aim_direction(p: &Player, hit: Option<Vec3>) -> Option<(f32, f32)> {
    let hit = hit?;
    let dx = hit.x - p.x;
    let dy = hit.z - p.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 {
        None
    } else {
        Some((dx / len, dy / len))
    }
}

/// Build this frame's `Command`s from the resolved aim plus live device state.
/// Reads Bevy input handles directly; `wheel_y` is the per-frame accumulated
/// `MouseWheel` delta (the system reads the event stream and sums it).
pub fn collect_commands(
    aim: Option<(f32, f32)>,
    pad: &PadInput,
    keys: &ButtonInput<KeyCode>,
    mouse_buttons: &ButtonInput<MouseButton>,
    wheel_y: f32,
) -> Vec<Command> {
    let mut cmds = Vec::new();

    // Movement: the gamepad left stick (analog) takes priority; else WASD/arrows.
    if let Some((dx, dy)) = pad.move_dir.or_else(|| keyboard_move(keys)) {
        cmds.push(Command::Move { dx, dy });
    }

    // Fire — continuous while the right trigger, LMB, or space is held. World
    // enforces the cooldown.
    let fire_held =
        pad.fire || mouse_buttons.pressed(MouseButton::Left) || keys.pressed(KeyCode::Space);
    if fire_held && let Some((adx, ady)) = aim {
        cmds.push(Command::Fire { dx: adx, dy: ady });
    }

    // Weapon switching (edge-triggered): number keys pick a rack slot directly,
    // Q/E and the mouse wheel cycle. The World no-ops out-of-range slots and
    // single-weapon cycles, so we can emit freely.
    for (key, slot) in [
        (KeyCode::Digit1, 0),
        (KeyCode::Digit2, 1),
        (KeyCode::Digit3, 2),
        (KeyCode::Digit4, 3),
    ] {
        if keys.just_pressed(key) {
            cmds.push(Command::SwitchWeapon { slot });
        }
    }
    if keys.just_pressed(KeyCode::KeyE) || wheel_y < 0.0 || pad.cycle_next {
        cmds.push(Command::CycleWeapon { dir: 1 });
    }
    if keys.just_pressed(KeyCode::KeyQ) || wheel_y > 0.0 || pad.cycle_prev {
        cmds.push(Command::CycleWeapon { dir: -1 });
    }

    cmds
}

/// Normalized WASD/arrow movement direction, or `None` if no key is held. `dy`
/// is along world Z, so W (−dy) moves "up"/north on screen.
fn keyboard_move(keys: &ButtonInput<KeyCode>) -> Option<(f32, f32)> {
    let mut dx = 0.0_f32;
    let mut dy = 0.0_f32;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        dy -= 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        dy += 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        dx -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        dx += 1.0;
    }
    if dx != 0.0 || dy != 0.0 {
        let len = (dx * dx + dy * dy).sqrt();
        Some((dx / len, dy / len))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player() -> Player {
        Player {
            x: 10.0,
            y: 10.0,
            vx: 0.0,
            vy: 0.0,
            max_life: 100.0,
            current_life: 100.0,
        }
    }

    fn pad(aim_dir: Option<(f32, f32)>, move_dir: Option<(f32, f32)>) -> PadInput {
        PadInput {
            move_dir,
            aim_dir,
            ..PadInput::default()
        }
    }

    // Right-stick aim claims the source and points where the stick points.
    #[test]
    fn pad_aim_claims_source() {
        let mut s = AimState::new((0.0, 0.0));
        let f = s.update(&player(), &pad(Some((1.0, 0.0)), None), (0.0, 0.0), None);
        assert_eq!(s.source, AimSource::Pad);
        assert_eq!(f.dir, Some((1.0, 0.0)));
    }

    // With the stick re-centered (no pad input) and no mouse movement, the last
    // pad heading is held rather than snapping back to the mouse.
    #[test]
    fn pad_heading_held_on_recenter() {
        let mut s = AimState::new((0.0, 0.0));
        s.update(&player(), &pad(Some((0.0, 1.0)), None), (0.0, 0.0), None);
        let f = s.update(&player(), &pad(None, None), (0.0, 0.0), None);
        assert_eq!(s.source, AimSource::Pad);
        assert_eq!(f.dir, Some((0.0, 1.0)));
    }

    // Left-stick movement (no aim) faces the movement direction.
    #[test]
    fn pad_move_drives_facing() {
        let mut s = AimState::new((0.0, 0.0));
        let f = s.update(&player(), &pad(None, Some((3.0, 4.0))), (0.0, 0.0), None);
        assert_eq!(s.source, AimSource::Pad);
        let (dx, dy) = f.dir.unwrap();
        assert!((dx - 0.6).abs() < 1e-5 && (dy - 0.8).abs() < 1e-5);
    }

    // After the pad latches, real mouse movement (> 1px) reclaims the source
    // and aim follows the ground hit.
    #[test]
    fn mouse_movement_reclaims_source() {
        let mut s = AimState::new((0.0, 0.0));
        s.update(&player(), &pad(Some((1.0, 0.0)), None), (0.0, 0.0), None);
        let hit = Vec3::new(13.0, 0.0, 14.0); // dx=3, dz=4 → (0.6, 0.8)
        let f = s.update(&player(), &pad(None, None), (50.0, 50.0), Some(hit));
        assert_eq!(s.source, AimSource::Mouse);
        let (dx, dy) = f.dir.unwrap();
        assert!((dx - 0.6).abs() < 1e-5 && (dy - 0.8).abs() < 1e-5);
    }

    // Sub-pixel jitter (≤ 1px) does not count as mouse movement, so the pad
    // keeps the source.
    #[test]
    fn subpixel_mouse_jitter_ignored() {
        let mut s = AimState::new((0.0, 0.0));
        s.update(&player(), &pad(Some((1.0, 0.0)), None), (0.0, 0.0), None);
        let f = s.update(&player(), &pad(None, None), (0.5, 0.5), None);
        assert_eq!(s.source, AimSource::Pad);
        assert_eq!(f.dir, Some((1.0, 0.0))); // held heading, not the mouse
    }
}
