//! Input layer: turn raw devices (keyboard, mouse, gamepad) into gameplay
//! intent. The aim-source latch lives here as `AimState` — the one piece of
//! per-frame input state that carries across frames — kept pure (it takes the
//! mouse position as an argument rather than reading the macroquad global) so
//! the mouse-vs-pad switching logic is unit-testable headlessly.

use crate::pad::PadInput;
use h2b_game::{Command, Player};
use macroquad::prelude::*;

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
    /// point ahead of the player when aiming on the stick.
    pub hit: Option<Vec3>,
}

/// The mouse-vs-pad aim latch plus the held heading. Carried across frames by
/// the main loop; one `update` per frame produces that frame's `AimFrame`.
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
                dir.map(|(ax, ay)| vec3(player.x + ax * 6.0, 0.0, player.y + ay * 6.0))
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

/// Collect this frame's `Command`s from the resolved aim plus live device
/// state (gamepad, mouse buttons, keyboard).
pub fn collect_input(aim: Option<(f32, f32)>, pad: &PadInput) -> Vec<Command> {
    let mut cmds = Vec::new();

    // Movement: the gamepad left stick (analog) takes priority; else WASD/arrows.
    if let Some((dx, dy)) = pad.move_dir.or_else(keyboard_move) {
        cmds.push(Command::Move { dx, dy });
    }

    // Fire — continuous while the right trigger, LMB, or space is held. World
    // enforces the cooldown.
    let fire_held =
        pad.fire || is_mouse_button_down(MouseButton::Left) || is_key_down(KeyCode::Space);
    if fire_held && let Some((adx, ady)) = aim {
        cmds.push(Command::Fire { dx: adx, dy: ady });
    }

    // Weapon switching (edge-triggered): number keys pick a rack slot directly,
    // Q/E and the mouse wheel cycle. The World no-ops out-of-range slots and
    // single-weapon cycles, so we can emit freely.
    for (key, slot) in [
        (KeyCode::Key1, 0),
        (KeyCode::Key2, 1),
        (KeyCode::Key3, 2),
        (KeyCode::Key4, 3),
    ] {
        if is_key_pressed(key) {
            cmds.push(Command::SwitchWeapon { slot });
        }
    }
    let wheel = mouse_wheel().1;
    if is_key_pressed(KeyCode::E) || wheel < 0.0 || pad.cycle_next {
        cmds.push(Command::CycleWeapon { dir: 1 });
    }
    if is_key_pressed(KeyCode::Q) || wheel > 0.0 || pad.cycle_prev {
        cmds.push(Command::CycleWeapon { dir: -1 });
    }

    cmds
}

/// Normalized WASD/arrow movement direction, or `None` if no key is held. `dy`
/// is along world Z, so W (−dy) moves "up"/north on screen.
fn keyboard_move() -> Option<(f32, f32)> {
    let mut dx = 0.0_f32;
    let mut dy = 0.0_f32;
    if is_key_down(KeyCode::W) || is_key_down(KeyCode::Up) {
        dy -= 1.0;
    }
    if is_key_down(KeyCode::S) || is_key_down(KeyCode::Down) {
        dy += 1.0;
    }
    if is_key_down(KeyCode::A) || is_key_down(KeyCode::Left) {
        dx -= 1.0;
    }
    if is_key_down(KeyCode::D) || is_key_down(KeyCode::Right) {
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
        let hit = vec3(13.0, 0.0, 14.0); // dx=3, dz=4 → (0.6, 0.8)
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
