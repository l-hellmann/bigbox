//! Flow-field pathfinding (Dijkstra map). Computed once per goal change —
//! typically when the player moves between tiles — by a 4-connected BFS
//! from the goal across `Tile::Floor`. Each cell stores the integer step
//! count back to the goal; walls and unreachable tiles stay at
//! `UNREACHABLE` (`u32::MAX`).
//!
//! Enemies follow the gradient: `next_step_from(x, y)` returns whichever
//! neighbor has a smaller distance, in O(1). That's the BoxHead-style pile
//! of zombies converging on the player — no per-enemy pathfinding work.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::{Map, Tile};

/// Sentinel for "no path to goal" (walls, unreachable rooms).
pub const UNREACHABLE: u32 = u32::MAX;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowField {
    pub width: u32,
    pub height: u32,
    pub goal: (u32, u32),
    /// Row-major. `distances[y * width + x]` = step count back to `goal`,
    /// or `UNREACHABLE` if not connected via `Floor` tiles.
    pub distances: Vec<u32>,
}

impl FlowField {
    /// 4-connected BFS from `goal` across `Floor` tiles. Walls block; the
    /// goal itself is seeded at distance 0 regardless of its own tile
    /// (game code always passes a Floor goal, but the algorithm doesn't
    /// require it). Returns a fully-populated field; never fails.
    pub fn compute(map: &Map, goal: (u32, u32)) -> Self {
        let total = (map.width * map.height) as usize;
        let mut distances = vec![UNREACHABLE; total];
        let mut queue: VecDeque<(u32, u32)> = VecDeque::new();

        if goal.0 < map.width && goal.1 < map.height {
            distances[(goal.1 * map.width + goal.0) as usize] = 0;
            queue.push_back(goal);
        }

        while let Some((x, y)) = queue.pop_front() {
            let d = distances[(y * map.width + x) as usize];
            for &(dx, dy) in &NEIGHBORS_4 {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0
                    || ny < 0
                    || nx >= map.width as i32
                    || ny >= map.height as i32
                {
                    continue;
                }
                let (nx, ny) = (nx as u32, ny as u32);
                if map.tile_at(nx, ny) != Tile::Floor {
                    continue;
                }
                let nidx = (ny * map.width + nx) as usize;
                let next_d = d.saturating_add(1);
                if distances[nidx] > next_d {
                    distances[nidx] = next_d;
                    queue.push_back((nx, ny));
                }
            }
        }

        Self {
            width: map.width,
            height: map.height,
            goal,
            distances,
        }
    }

    pub fn distance_at(&self, x: u32, y: u32) -> u32 {
        if x >= self.width || y >= self.height {
            return UNREACHABLE;
        }
        self.distances[(y * self.width + x) as usize]
    }

    /// Returns the neighbor tile (4-connected) closest to the goal — the
    /// next step an enemy at `(x, y)` should take. Ties resolve in
    /// `NEIGHBORS_4` iteration order (N, E, S, W). Returns `None` at the
    /// goal itself, when standing on an unreachable tile, or when no
    /// neighbor has a strictly smaller distance.
    pub fn next_step_from(&self, x: u32, y: u32) -> Option<(u32, u32)> {
        let here = self.distance_at(x, y);
        if here == UNREACHABLE || here == 0 {
            return None;
        }
        let mut best: Option<(u32, u32, u32)> = None;
        for &(dx, dy) in &NEIGHBORS_4 {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 {
                continue;
            }
            let (nx, ny) = (nx as u32, ny as u32);
            let nd = self.distance_at(nx, ny);
            if nd >= here {
                continue;
            }
            match best {
                None => best = Some((nx, ny, nd)),
                Some((_, _, bd)) if nd < bd => best = Some((nx, ny, nd)),
                _ => {}
            }
        }
        best.map(|(x, y, _)| (x, y))
    }

    /// Smooth descent direction (normalized) from a **continuous** position
    /// toward the goal — the fluid alternative to [`next_step_from`]. Instead of
    /// snapping to a 4-neighbor tile (which makes enemies hop center-to-center
    /// in a grid staircase), this bilinearly samples the distance field and
    /// follows its negative gradient, so motion is continuous and 8-directional.
    ///
    /// Walls / unreachable / out-of-bounds cells are sampled as a penalty
    /// *above* the local distance, so the gradient is pushed away from them:
    /// enemies bend **around** pillars and corners rather than snagging on or
    /// clipping into them. Symmetric walls (a 1-wide corridor) cancel out, so
    /// the enemy stays centered and walks straight down it.
    ///
    /// Returns `None` at the goal tile or on an unreachable position — the
    /// caller falls back to homing straight at the goal.
    pub fn steer_from(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let here = self.distance_at(x as u32, y as u32);
        if here == UNREACHABLE || here == 0 {
            return None;
        }
        let baseline = here as f32;
        const EPS: f32 = 0.5;
        // Central difference of the (penalty-substituted) distance field. The
        // negative gradient — sample(behind) − sample(ahead) — points downhill.
        let gx = self.sample(x - EPS, y, baseline) - self.sample(x + EPS, y, baseline);
        let gy = self.sample(x, y - EPS, baseline) - self.sample(x, y + EPS, baseline);
        let len = (gx * gx + gy * gy).sqrt();
        if len < 1e-4 {
            return None;
        }
        Some((gx / len, gy / len))
    }

    /// Normalized direction toward the discrete next step from a continuous
    /// position. This is [`steer_from`]'s fallback at a **symmetric saddle** —
    /// e.g. dead-center on the approach to a pillar, where both detour routes
    /// are equal length and the smooth gradient cancels to zero. The discrete
    /// [`next_step_from`] tiebreak picks a side deterministically, nudging the
    /// enemy off the saddle so the smooth gradient can take over next tick.
    pub fn next_step_dir(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let (nx, ny) = self.next_step_from(x as u32, y as u32)?;
        let dx = (nx as f32 + 0.5) - x;
        let dy = (ny as f32 + 0.5) - y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-4 {
            None
        } else {
            Some((dx / len, dy / len))
        }
    }

    /// Bilinearly sample the distance field at a continuous position, treating
    /// each tile's distance as a sample at its center `(i + 0.5)`. Walls /
    /// unreachable / OOB read as `baseline + WALL_PENALTY` so they repel the
    /// gradient in [`steer_from`].
    fn sample(&self, x: f32, y: f32, baseline: f32) -> f32 {
        /// How far above the local distance a wall reads. Tuned so wall
        /// repulsion is comparable to the one-tile goal pull (distances step by
        /// 1): enough to bend a path around a corner, not so much that enemies
        /// refuse to enter gaps.
        const WALL_PENALTY: f32 = 2.5;
        let penalty = baseline + WALL_PENALTY;
        let at = |ix: i32, iy: i32| -> f32 {
            if ix < 0 || iy < 0 || ix >= self.width as i32 || iy >= self.height as i32 {
                return penalty;
            }
            let d = self.distances[(iy as u32 * self.width + ix as u32) as usize];
            if d == UNREACHABLE { penalty } else { d as f32 }
        };
        // Shift by the tile-center offset so integer parts index tiles.
        let gx = x - 0.5;
        let gy = y - 0.5;
        let x0 = gx.floor();
        let y0 = gy.floor();
        let fx = gx - x0;
        let fy = gy - y0;
        let (x0, y0) = (x0 as i32, y0 as i32);
        let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
        let bot = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
        top * (1.0 - fy) + bot * fy
    }
}

/// N, E, S, W in that iteration order (used for `next_step_from`
/// tiebreaking, so a future change to that order is observable).
const NEIGHBORS_4: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MapParams, generate_bsp};

    /// Hand-rolled 5×3 map for predictable distance tests:
    ///   #####
    ///   #...#
    ///   #####
    fn corridor_map() -> Map {
        let w = 5;
        let h = 3;
        let mut tiles = vec![Tile::Wall; (w * h) as usize];
        // Floor along the middle row, cols 1..=3
        for x in 1..=3u32 {
            tiles[(w + x) as usize] = Tile::Floor;
        }
        Map {
            width: w,
            height: h,
            tiles,
            rooms: vec![],
            seed: 0,
            player_spawn: (1, 1),
        }
    }

    /// 7×3 with two floor segments split by a wall column:
    ///   #######
    ///   #.###.#
    ///   #######
    fn disconnected_map() -> Map {
        let w = 7;
        let h = 3;
        let mut tiles = vec![Tile::Wall; (w * h) as usize];
        tiles[(w + 1) as usize] = Tile::Floor;
        tiles[(w + 5) as usize] = Tile::Floor;
        Map {
            width: w,
            height: h,
            tiles,
            rooms: vec![],
            seed: 0,
            player_spawn: (1, 1),
        }
    }

    #[test]
    fn goal_distance_is_zero() {
        let map = corridor_map();
        let ff = FlowField::compute(&map, (2, 1));
        assert_eq!(ff.distance_at(2, 1), 0);
    }

    #[test]
    fn direct_neighbors_of_goal_are_distance_one() {
        let map = corridor_map();
        let ff = FlowField::compute(&map, (2, 1));
        assert_eq!(ff.distance_at(1, 1), 1);
        assert_eq!(ff.distance_at(3, 1), 1);
        // Walls above/below the goal stay unreachable.
        assert_eq!(ff.distance_at(2, 0), UNREACHABLE);
        assert_eq!(ff.distance_at(2, 2), UNREACHABLE);
    }

    #[test]
    fn walls_remain_unreachable() {
        let map = corridor_map();
        let ff = FlowField::compute(&map, (1, 1));
        // Top-left wall corner.
        assert_eq!(ff.distance_at(0, 0), UNREACHABLE);
        // The full bottom row.
        for x in 0..map.width {
            assert_eq!(ff.distance_at(x, 2), UNREACHABLE);
        }
    }

    #[test]
    fn disconnected_floor_has_no_path() {
        let map = disconnected_map();
        let ff = FlowField::compute(&map, (1, 1));
        assert_eq!(ff.distance_at(1, 1), 0);
        // The other floor segment is walled off.
        assert_eq!(ff.distance_at(5, 1), UNREACHABLE);
    }

    #[test]
    fn next_step_descends_distance() {
        let map = corridor_map();
        let ff = FlowField::compute(&map, (3, 1));
        // From (1,1) we should step toward (2,1) — closer to the goal.
        assert_eq!(ff.next_step_from(1, 1), Some((2, 1)));
        assert_eq!(ff.next_step_from(2, 1), Some((3, 1)));
        // Already at the goal — no step needed.
        assert_eq!(ff.next_step_from(3, 1), None);
        // Standing on a wall — nothing to do.
        assert_eq!(ff.next_step_from(0, 0), None);
    }

    #[test]
    fn steer_descends_toward_goal() {
        let map = corridor_map(); // floor row y=1, x=1..=3
        let ff = FlowField::compute(&map, (3, 1));
        // From the west end, smooth steering should head east (+x) toward goal,
        // and the symmetric N/S corridor walls keep it vertically centered.
        let (dx, dy) = ff.steer_from(1.5, 1.5).unwrap();
        assert!(dx > 0.5, "should steer east toward goal, got dx={dx}");
        assert!(dy.abs() < 0.2, "corridor should keep it centered, got dy={dy}");
    }

    #[test]
    fn steer_is_none_at_goal_and_off_path() {
        let map = corridor_map();
        let ff = FlowField::compute(&map, (2, 1));
        // On the goal tile there's no descent direction.
        assert!(ff.steer_from(2.5, 1.5).is_none());
        // On an unreachable (wall) position, also none.
        assert!(ff.steer_from(0.5, 0.5).is_none());
    }

    /// 5×5 room, walls on the border plus a single pillar at (2,2); goal (3,2).
    /// The only routes from the west side to the goal go around the pillar.
    fn pillar_map() -> Map {
        let w = 5;
        let h = 5;
        let mut tiles = vec![Tile::Floor; (w * h) as usize];
        for x in 0..w {
            tiles[x as usize] = Tile::Wall;
            tiles[((h - 1) * w + x) as usize] = Tile::Wall;
        }
        for y in 0..h {
            tiles[(y * w) as usize] = Tile::Wall;
            tiles[(y * w + w - 1) as usize] = Tile::Wall;
        }
        tiles[(2 * w + 2) as usize] = Tile::Wall; // pillar
        Map {
            width: w,
            height: h,
            tiles,
            rooms: vec![],
            seed: 0,
            player_spawn: (3, 2),
        }
    }

    #[test]
    fn steer_deflects_off_center_near_pillar() {
        let ff = FlowField::compute(&pillar_map(), (3, 2));
        // Just north of the symmetric line, west of the pillar: the wall
        // penalty pulls the steer further north to round the pillar rather than
        // straight east into it.
        let (_, dy) = ff.steer_from(1.5, 2.2).unwrap();
        assert!(dy < 0.0, "should deflect north around the pillar, got dy={dy}");
    }

    #[test]
    fn steer_saddle_falls_back_to_discrete_step() {
        let ff = FlowField::compute(&pillar_map(), (3, 2));
        // Dead-center on the approach both detours are equal, so the smooth
        // gradient vanishes — and the discrete next-step breaks the tie.
        assert!(ff.steer_from(1.5, 2.5).is_none(), "symmetric saddle → no smooth gradient");
        assert!(ff.next_step_dir(1.5, 2.5).is_some(), "discrete step picks a side");
    }

    #[test]
    fn determinism_same_map_and_goal() {
        let map = corridor_map();
        let a = FlowField::compute(&map, (2, 1));
        let b = FlowField::compute(&map, (2, 1));
        assert_eq!(a.distances, b.distances);
    }

    #[test]
    fn every_floor_on_bsp_map_is_reachable_from_spawn() {
        // Doubles as a procgen connectivity property test: BSP corridors
        // should leave no isolated Floor tile.
        for seed in [1u64, 7, 42, 99, 12345] {
            let map = generate_bsp(&MapParams {
                seed,
                ..Default::default()
            });
            let ff = FlowField::compute(&map, map.player_spawn);
            for y in 0..map.height {
                for x in 0..map.width {
                    if map.tile_at(x, y) == Tile::Floor {
                        assert_ne!(
                            ff.distance_at(x, y),
                            UNREACHABLE,
                            "seed={seed} tile ({x},{y}) is Floor but unreachable from spawn"
                        );
                    }
                }
            }
        }
    }
}
