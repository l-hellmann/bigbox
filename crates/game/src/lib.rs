//! Game-state primitives for the runtime layer. **No rendering or input
//! plumbing in here** — that lives in `main.rs` next door, behind macroquad.
//! Keeping the state/logic side library-shaped means:
//!
//! 1. It can be unit-tested without spinning up a window.
//! 2. It can later run server-side inside a SpacetimeDB reducer module
//!    unchanged (CLAUDE.md multiplayer posture, habit #3).
//! 3. The runtime layer never gets to mutate state directly — input is
//!    funneled through `Command` variants which are trivially serializable
//!    (habit #1).

use h2b_procgen::{Map, Tile};

#[derive(Debug, Clone, Copy)]
pub struct Player {
    /// Position in **tile coordinates**, with sub-tile precision. The
    /// renderer multiplies by tile size to get screen pixels.
    pub x: f32,
    pub y: f32,
}

pub struct World {
    pub map: Map,
    pub player: Player,
}

impl World {
    pub fn new(map: Map) -> Self {
        let (sx, sy) = map.player_spawn;
        Self {
            player: Player {
                x: sx as f32 + 0.5,
                y: sy as f32 + 0.5,
            },
            map,
        }
    }
}

/// Commands are the **only** way to mutate the world. Input collectors emit
/// these; tests pass them in directly. Designed to serialize cleanly so the
/// same command stream can drive a coop reducer when that lands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// `(dx, dy)` is the direction vector — magnitude should be ≤ 1.0
    /// (caller normalizes). `World::apply` multiplies by `dt × speed` to
    /// get the per-frame displacement, so the command itself is
    /// frame-rate-independent.
    Move { dx: f32, dy: f32 },
}

impl World {
    /// Apply one command to the world. Movement uses **per-axis collision**
    /// so the player slides along walls instead of getting hard-stopped on
    /// diagonal approaches.
    pub fn apply(&mut self, cmd: Command, dt: f32, speed: f32) {
        match cmd {
            Command::Move { dx, dy } => {
                let step = speed * dt;
                let nx = self.player.x + dx * step;
                if self.can_stand(nx, self.player.y) {
                    self.player.x = nx;
                }
                let ny = self.player.y + dy * step;
                if self.can_stand(self.player.x, ny) {
                    self.player.y = ny;
                }
            }
        }
    }

    fn can_stand(&self, x: f32, y: f32) -> bool {
        if x < 0.0 || y < 0.0 {
            return false;
        }
        let tx = x as u32;
        let ty = y as u32;
        matches!(self.map.tile_at(tx, ty), Tile::Floor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2b_procgen::{MapParams, generate_bsp};

    fn world_at_seed(seed: u64) -> World {
        World::new(generate_bsp(&MapParams {
            seed,
            ..Default::default()
        }))
    }

    #[test]
    fn spawn_is_on_a_floor_tile() {
        let w = world_at_seed(42);
        assert!(w.can_stand(w.player.x, w.player.y), "spawn must be standable");
    }

    #[test]
    fn move_command_advances_player_on_floor() {
        let mut w = world_at_seed(42);
        let (x0, y0) = (w.player.x, w.player.y);
        // Speed 10, dt 0.1 = 1 tile of movement; tiny direction toward +x.
        w.apply(Command::Move { dx: 1.0, dy: 0.0 }, 0.1, 10.0);
        // Either moved or stayed (blocked by wall), but didn't tunnel.
        assert!(w.can_stand(w.player.x, w.player.y));
        // y unchanged for a pure-x move.
        assert!((w.player.y - y0).abs() < 1e-6);
        // If the +x neighbor is floor, we should have moved.
        if w.can_stand(x0 + 1.0, y0) {
            assert!(w.player.x > x0);
        }
    }

    #[test]
    fn wall_blocks_movement_on_the_blocked_axis_only() {
        // Hand-rolled tiny map: 3×3 with a single floor cell at (1,1)
        //   ###
        //   #.#
        //   ###
        let w = 3;
        let h = 3;
        let mut tiles = vec![Tile::Wall; (w * h) as usize];
        tiles[(1 * w + 1) as usize] = Tile::Floor;
        let map = Map {
            width: w,
            height: h,
            tiles,
            rooms: vec![],
            seed: 0,
            player_spawn: (1, 1),
        };
        let mut world = World::new(map);
        let (x0, y0) = (world.player.x, world.player.y);
        // Try to move diagonally into the corner — both axes blocked.
        world.apply(Command::Move { dx: 1.0, dy: 1.0 }, 1.0, 10.0);
        assert!((world.player.x - x0).abs() < 1e-6, "x should not advance into wall");
        assert!((world.player.y - y0).abs() < 1e-6, "y should not advance into wall");
    }

    #[test]
    fn per_axis_collision_allows_sliding() {
        // Map with a corridor along y (a column of floor):
        //   #.#
        //   #.#
        //   #.#
        let w = 3;
        let h = 3;
        let mut tiles = vec![Tile::Wall; (w * h) as usize];
        for y in 0..h {
            tiles[(y * w + 1) as usize] = Tile::Floor;
        }
        let map = Map {
            width: w,
            height: h,
            tiles,
            rooms: vec![],
            seed: 0,
            player_spawn: (1, 1),
        };
        let mut world = World::new(map);
        // Diagonal command: +x blocked (wall), +y allowed (open corridor).
        // The y component should still apply.
        world.apply(Command::Move { dx: 1.0, dy: 1.0 }, 0.05, 10.0);
        assert!((world.player.x - 1.5).abs() < 1e-6, "x stays at center of corridor");
        assert!(world.player.y > 1.5, "slid down despite x being walled");
    }

    #[test]
    fn out_of_bounds_is_not_standable() {
        let w = world_at_seed(42);
        assert!(!w.can_stand(-0.1, 5.0));
        assert!(!w.can_stand(5.0, -0.1));
        assert!(!w.can_stand(w.map.width as f32 + 1.0, 5.0));
    }

    #[test]
    fn applying_same_commands_is_deterministic() {
        let mut a = world_at_seed(42);
        let mut b = world_at_seed(42);
        let cmds = [
            Command::Move { dx: 1.0, dy: 0.0 },
            Command::Move { dx: 0.0, dy: 1.0 },
            Command::Move { dx: -1.0, dy: 0.0 },
        ];
        for cmd in cmds {
            a.apply(cmd, 0.05, 6.0);
            b.apply(cmd, 0.05, 6.0);
        }
        assert!((a.player.x - b.player.x).abs() < 1e-6);
        assert!((a.player.y - b.player.y).abs() < 1e-6);
    }
}
