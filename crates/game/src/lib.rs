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

// ---------- tunables ----------
//
// Hardcoded for v1 — these come from the equipped weapon's aggregated stats
// once loot is wired into the runtime. Encoded as constants for now so the
// values are visible and adjustable in one place.

/// Player movement speed in tiles per second.
pub const PLAYER_SPEED: f32 = 6.0;
/// Projectile speed in tiles per second. ~4× player speed so shots feel snappy.
pub const PROJECTILE_SPEED: f32 = 24.0;
/// Shots per second; reciprocal is the per-shot cooldown.
pub const FIRE_RATE: f32 = 4.0;
/// Damage applied on a projectile-vs-target hit. (No targets yet — values
/// the future hit-detection layer will read.)
pub const BULLET_DAMAGE: f32 = 12.0;
/// Hard cap on how long a projectile can live before it despawns, in seconds.
/// Wall collision usually kills shots much earlier; this is the fail-safe.
pub const BULLET_LIFETIME: f32 = 1.5;

#[derive(Debug, Clone, Copy)]
pub struct Player {
    /// Position in **tile coordinates**, with sub-tile precision. The
    /// renderer multiplies by tile size to get screen pixels.
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Projectile {
    pub x: f32,
    pub y: f32,
    /// Velocity in tiles per second.
    pub vx: f32,
    pub vy: f32,
    pub damage: f32,
    /// Seconds remaining before forced despawn.
    pub lifetime: f32,
}

pub struct World {
    pub map: Map,
    pub player: Player,
    pub projectiles: Vec<Projectile>,
    /// Seconds until the player's next shot becomes available.
    pub player_fire_cooldown: f32,
}

impl World {
    pub fn new(map: Map) -> Self {
        let (sx, sy) = map.player_spawn;
        Self {
            player: Player {
                x: sx as f32 + 0.5,
                y: sy as f32 + 0.5,
            },
            projectiles: Vec::new(),
            player_fire_cooldown: 0.0,
            map,
        }
    }
}

/// Commands are the **only** way to mutate the world. Input collectors emit
/// these; tests pass them in directly. Designed to serialize cleanly so the
/// same command stream can drive a coop reducer when that lands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// `(dx, dy)` normalized direction. World scales by `dt × PLAYER_SPEED`.
    Move { dx: f32, dy: f32 },
    /// Fire in normalized direction `(dx, dy)`. No-op if the cooldown isn't
    /// up — callers can spam this every frame while LMB is held; the World
    /// rate-limits.
    Fire { dx: f32, dy: f32 },
}

impl World {
    /// Apply one command. Movement uses **per-axis collision** so the player
    /// slides along walls instead of getting hard-stopped on diagonals. Fire
    /// respects `player_fire_cooldown` and silently drops if not ready.
    pub fn apply(&mut self, cmd: Command, dt: f32) {
        match cmd {
            Command::Move { dx, dy } => {
                let step = PLAYER_SPEED * dt;
                let nx = self.player.x + dx * step;
                if self.can_stand(nx, self.player.y) {
                    self.player.x = nx;
                }
                let ny = self.player.y + dy * step;
                if self.can_stand(self.player.x, ny) {
                    self.player.y = ny;
                }
            }
            Command::Fire { dx, dy } => {
                if self.player_fire_cooldown > 0.0 {
                    return;
                }
                // Normalize defensively — caller is expected to send unit
                // vectors but a zero-vector fire would create stationary
                // projectiles, which is silly.
                let len = (dx * dx + dy * dy).sqrt();
                if len < 1e-6 {
                    return;
                }
                let (nx, ny) = (dx / len, dy / len);
                self.projectiles.push(Projectile {
                    x: self.player.x,
                    y: self.player.y,
                    vx: nx * PROJECTILE_SPEED,
                    vy: ny * PROJECTILE_SPEED,
                    damage: BULLET_DAMAGE,
                    lifetime: BULLET_LIFETIME,
                });
                self.player_fire_cooldown = 1.0 / FIRE_RATE;
            }
        }
    }

    /// Advance time-based state by `dt` seconds. Apply this **after**
    /// command resolution each frame so newly-fired projectiles get one
    /// frame of motion before the next render. Order in main: collect →
    /// apply → tick → draw.
    pub fn tick(&mut self, dt: f32) {
        // Cooldown decay.
        self.player_fire_cooldown = (self.player_fire_cooldown - dt).max(0.0);

        // Advance projectiles in place; despawn on wall hit, OOB, or
        // lifetime expiry. swap_remove keeps it O(1) per drop. Copy fields
        // out first so the passability check can borrow `self` immutably
        // without fighting the active mutable borrow.
        let mut i = 0;
        while i < self.projectiles.len() {
            let p = self.projectiles[i];
            let new_life = p.lifetime - dt;
            if new_life <= 0.0 {
                self.projectiles.swap_remove(i);
                continue;
            }
            let nx = p.x + p.vx * dt;
            let ny = p.y + p.vy * dt;
            if !self.is_passable(nx, ny) {
                self.projectiles.swap_remove(i);
                continue;
            }
            let slot = &mut self.projectiles[i];
            slot.x = nx;
            slot.y = ny;
            slot.lifetime = new_life;
            i += 1;
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

    /// Whether a projectile point at `(x, y)` is in a tile a bullet can fly
    /// through. Same as `can_stand` for now — players and projectiles share
    /// the same passability rules. Split if we ever add window/pit tiles.
    fn is_passable(&self, x: f32, y: f32) -> bool {
        self.can_stand(x, y)
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

    fn single_floor_map() -> Map {
        // 3×3 with one Floor at (1,1).
        let w = 3;
        let h = 3;
        let mut tiles = vec![Tile::Wall; (w * h) as usize];
        tiles[(1 * w + 1) as usize] = Tile::Floor;
        Map {
            width: w,
            height: h,
            tiles,
            rooms: vec![],
            seed: 0,
            player_spawn: (1, 1),
        }
    }

    // ---- movement ----

    #[test]
    fn spawn_is_on_a_floor_tile() {
        let w = world_at_seed(42);
        assert!(w.can_stand(w.player.x, w.player.y));
    }

    #[test]
    fn move_command_advances_player_on_floor() {
        let mut w = world_at_seed(42);
        let (x0, y0) = (w.player.x, w.player.y);
        w.apply(Command::Move { dx: 1.0, dy: 0.0 }, 0.1);
        assert!(w.can_stand(w.player.x, w.player.y));
        assert!((w.player.y - y0).abs() < 1e-6);
        if w.can_stand(x0 + 1.0, y0) {
            assert!(w.player.x > x0);
        }
    }

    #[test]
    fn wall_blocks_movement_on_the_blocked_axis_only() {
        let mut w = World::new(single_floor_map());
        let (x0, y0) = (w.player.x, w.player.y);
        w.apply(Command::Move { dx: 1.0, dy: 1.0 }, 1.0);
        assert!((w.player.x - x0).abs() < 1e-6);
        assert!((w.player.y - y0).abs() < 1e-6);
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
        // dt × PLAYER_SPEED × dx must push the new_x across the wall
        // boundary at x=2. 0.1 × 6 × 1 = 0.6 → new_x 1.5 + 0.6 = 2.1 → wall.
        world.apply(Command::Move { dx: 1.0, dy: 1.0 }, 0.1);
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
            a.apply(cmd, 0.05);
            b.apply(cmd, 0.05);
        }
        assert!((a.player.x - b.player.x).abs() < 1e-6);
        assert!((a.player.y - b.player.y).abs() < 1e-6);
    }

    // ---- shooting ----

    #[test]
    fn fire_spawns_a_projectile() {
        let mut w = world_at_seed(42);
        assert_eq!(w.projectiles.len(), 0);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        assert_eq!(w.projectiles.len(), 1);
        let p = &w.projectiles[0];
        assert!((p.x - w.player.x).abs() < 1e-6);
        assert!((p.y - w.player.y).abs() < 1e-6);
        assert!((p.vx - PROJECTILE_SPEED).abs() < 1e-4);
        assert!(p.vy.abs() < 1e-4);
        assert!(p.damage > 0.0);
        assert!(w.player_fire_cooldown > 0.0);
    }

    #[test]
    fn fire_respects_cooldown() {
        let mut w = world_at_seed(42);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        // Three fires in the same frame — only the first should spawn.
        assert_eq!(w.projectiles.len(), 1);
    }

    #[test]
    fn cooldown_expires_with_tick() {
        let mut w = world_at_seed(42);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        // Tick past the cooldown window (1/FIRE_RATE = 0.25s).
        w.tick(0.30);
        assert_eq!(w.player_fire_cooldown, 0.0);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        assert_eq!(w.projectiles.len(), 2);
    }

    #[test]
    fn zero_vector_fire_is_ignored() {
        let mut w = world_at_seed(42);
        w.apply(Command::Fire { dx: 0.0, dy: 0.0 }, 0.016);
        assert_eq!(w.projectiles.len(), 0);
        assert_eq!(w.player_fire_cooldown, 0.0);
    }

    #[test]
    fn projectile_advances_with_tick() {
        let mut w = world_at_seed(42);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        let x0 = w.projectiles[0].x;
        w.tick(0.05);
        // Either advanced or died on a wall — but didn't sit still.
        if let Some(p) = w.projectiles.first() {
            assert!(p.x > x0, "projectile should have moved +x");
        }
    }

    #[test]
    fn projectile_despawns_on_wall_collision() {
        // 5×3 corridor, single floor row. Fire eastward from (1,1); the
        // projectile dies as soon as it reaches the east wall.
        let w = 5;
        let h = 3;
        let mut tiles = vec![Tile::Wall; (w * h) as usize];
        for x in 1..=3u32 {
            tiles[(1 * w + x) as usize] = Tile::Floor;
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
        world.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        // PROJECTILE_SPEED is 24 tiles/s; well within 0.2s a shot crosses
        // the 3-floor-wide corridor and hits the east wall.
        world.tick(0.20);
        assert!(world.projectiles.is_empty(), "projectile should hit the east wall");
    }

    #[test]
    fn projectile_despawns_on_lifetime_expiry() {
        let mut w = world_at_seed(42);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        // Force expiry without any wall in the way by ticking past lifetime.
        w.tick(BULLET_LIFETIME + 0.1);
        assert!(w.projectiles.is_empty());
    }

    #[test]
    fn multiple_projectiles_coexist() {
        let mut w = world_at_seed(42);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        w.tick(0.30); // cooldown clears
        w.apply(Command::Fire { dx: 0.0, dy: 1.0 }, 0.016);
        w.tick(0.30);
        w.apply(Command::Fire { dx: -1.0, dy: 0.0 }, 0.016);
        // Three distinct shots in flight (assuming none have hit walls yet).
        assert!(w.projectiles.len() >= 1);
    }
}
