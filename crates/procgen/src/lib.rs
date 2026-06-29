//! Procedural map generation. v1 ships a BSP generator — recursive space
//! partitioning, one room per leaf, L-corridors connecting sibling centers
//! on the way back up. Deterministic from a `u64` seed; renderer-agnostic.
//!
//! Biomes / room templates / variant tilesets are out of scope for the
//! first cut. The current model is a single tile palette (`Wall`/`Floor`)
//! with a player spawn — enough to feed an enemy pathfinder and let the
//! game-loop crate stand something up.

use rand::{Rng, SeedableRng, rngs::StdRng};
use serde::{Deserialize, Serialize};

pub mod flow;
pub mod spawn;
pub use flow::{FlowField, UNREACHABLE};
pub use spawn::pick_spawn_points;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tile {
    Wall,
    Floor,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Room {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Room {
    pub fn center(&self) -> (u32, u32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Map {
    pub width: u32,
    pub height: u32,
    /// Row-major. `tiles[y * width + x]`.
    pub tiles: Vec<Tile>,
    pub rooms: Vec<Room>,
    pub seed: u64,
    pub player_spawn: (u32, u32),
}

impl Map {
    pub fn tile_at(&self, x: u32, y: u32) -> Tile {
        if x >= self.width || y >= self.height {
            return Tile::Wall;
        }
        self.tiles[(y * self.width + x) as usize]
    }

    /// Render as a single string with `\n` row separators.
    /// `#` = wall, `.` = floor, `@` = player spawn.
    pub fn render_ascii(&self) -> String {
        let mut s = String::with_capacity(((self.width + 1) * self.height) as usize);
        for y in 0..self.height {
            for x in 0..self.width {
                let ch = if (x, y) == self.player_spawn {
                    '@'
                } else {
                    match self.tile_at(x, y) {
                        Tile::Wall => '#',
                        Tile::Floor => '.',
                    }
                };
                s.push(ch);
            }
            s.push('\n');
        }
        s
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MapParams {
    pub width: u32,
    pub height: u32,
    pub min_room_size: u32,
    pub max_room_size: u32,
    pub max_depth: u8,
    pub seed: u64,
}

impl Default for MapParams {
    fn default() -> Self {
        Self {
            width: 80,
            height: 40,
            min_room_size: 5,
            max_room_size: 12,
            max_depth: 5,
            seed: 0,
        }
    }
}

/// Generate a BSP-style map. Always succeeds — degenerate parameters
/// (tiny size, impossible splits) produce a minimal map rather than an
/// error, so the caller doesn't need a fallback path.
pub fn generate_bsp(params: &MapParams) -> Map {
    let mut rng = StdRng::seed_from_u64(params.seed);
    let total = (params.width * params.height) as usize;
    let mut tiles = vec![Tile::Wall; total];
    let mut rooms: Vec<Room> = Vec::new();

    let root = Rect {
        x: 0,
        y: 0,
        w: params.width,
        h: params.height,
    };
    bsp_split(&mut rng, &root, params, 0, &mut tiles, &mut rooms);

    let player_spawn = rooms.first().map(|r| r.center()).unwrap_or((0, 0));

    Map {
        width: params.width,
        height: params.height,
        tiles,
        rooms,
        seed: params.seed,
        player_spawn,
    }
}

// ---------- internals ----------

#[derive(Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// Returns the center coord of a representative room produced inside `rect`
/// (the leaf's room when at the bottom, or the first child's center when
/// splitting). Used by the caller to draw a corridor up the tree.
fn bsp_split<R: Rng + ?Sized>(
    rng: &mut R,
    rect: &Rect,
    params: &MapParams,
    depth: u8,
    tiles: &mut [Tile],
    rooms: &mut Vec<Room>,
) -> (u32, u32) {
    // Stop splitting if any further cut would violate min_room_size on
    // both sides, or if we hit the recursion cap.
    let min_cut = params.min_room_size + 2; // wall padding on both edges
    let too_shallow = depth >= params.max_depth;
    let too_narrow = rect.w < min_cut * 2 && rect.h < min_cut * 2;
    if too_shallow || too_narrow {
        let room = place_room(rng, rect, params);
        carve_room(&room, tiles, params.width);
        rooms.push(room);
        return room.center();
    }

    let split_horizontal = pick_split_direction(rng, rect);

    let (a, b) = if split_horizontal {
        let lo = rect.y + min_cut;
        let hi = rect.y + rect.h - min_cut;
        if hi < lo {
            // Can't split vertically — try horizontal one more time.
            return split_or_leaf(rng, rect, params, depth, tiles, rooms, false);
        }
        let split = rng.gen_range(lo..=hi);
        (
            Rect { x: rect.x, y: rect.y, w: rect.w, h: split - rect.y },
            Rect { x: rect.x, y: split, w: rect.w, h: rect.h - (split - rect.y) },
        )
    } else {
        let lo = rect.x + min_cut;
        let hi = rect.x + rect.w - min_cut;
        if hi < lo {
            return split_or_leaf(rng, rect, params, depth, tiles, rooms, true);
        }
        let split = rng.gen_range(lo..=hi);
        (
            Rect { x: rect.x, y: rect.y, w: split - rect.x, h: rect.h },
            Rect { x: split, y: rect.y, w: rect.w - (split - rect.x), h: rect.h },
        )
    };

    let ca = bsp_split(rng, &a, params, depth + 1, tiles, rooms);
    let cb = bsp_split(rng, &b, params, depth + 1, tiles, rooms);
    carve_corridor(ca, cb, tiles, params.width);

    ca
}

fn split_or_leaf<R: Rng + ?Sized>(
    rng: &mut R,
    rect: &Rect,
    params: &MapParams,
    depth: u8,
    tiles: &mut [Tile],
    rooms: &mut Vec<Room>,
    try_horizontal: bool,
) -> (u32, u32) {
    let min_cut = params.min_room_size + 2;
    let dim = if try_horizontal { rect.h } else { rect.w };
    if dim < min_cut * 2 {
        // Truly can't split — place a room and return.
        let room = place_room(rng, rect, params);
        carve_room(&room, tiles, params.width);
        rooms.push(room);
        return room.center();
    }
    // Caller already failed on the other axis; force this one.
    if try_horizontal {
        let lo = rect.y + min_cut;
        let hi = rect.y + rect.h - min_cut;
        let split = rng.gen_range(lo..=hi);
        let a = Rect { x: rect.x, y: rect.y, w: rect.w, h: split - rect.y };
        let b = Rect { x: rect.x, y: split, w: rect.w, h: rect.h - (split - rect.y) };
        let ca = bsp_split(rng, &a, params, depth + 1, tiles, rooms);
        let cb = bsp_split(rng, &b, params, depth + 1, tiles, rooms);
        carve_corridor(ca, cb, tiles, params.width);
        ca
    } else {
        let lo = rect.x + min_cut;
        let hi = rect.x + rect.w - min_cut;
        let split = rng.gen_range(lo..=hi);
        let a = Rect { x: rect.x, y: rect.y, w: split - rect.x, h: rect.h };
        let b = Rect { x: split, y: rect.y, w: rect.w - (split - rect.x), h: rect.h };
        let ca = bsp_split(rng, &a, params, depth + 1, tiles, rooms);
        let cb = bsp_split(rng, &b, params, depth + 1, tiles, rooms);
        carve_corridor(ca, cb, tiles, params.width);
        ca
    }
}

fn pick_split_direction<R: Rng + ?Sized>(rng: &mut R, rect: &Rect) -> bool {
    // Strongly bias against splits that would produce slivers. If the rect
    // is much wider than tall, split vertically; if taller, horizontally;
    // if roughly square, coinflip.
    let ratio = rect.w as f32 / rect.h as f32;
    if ratio > 1.25 {
        false
    } else if ratio < 0.8 {
        true
    } else {
        rng.r#gen::<bool>()
    }
}

fn place_room<R: Rng + ?Sized>(rng: &mut R, rect: &Rect, params: &MapParams) -> Room {
    // Leave 1-cell padding inside the rect so adjacent rooms can't merge.
    let inner_w = rect.w.saturating_sub(2);
    let inner_h = rect.h.saturating_sub(2);
    let w = clamp_size(rng, inner_w, params.min_room_size, params.max_room_size);
    let h = clamp_size(rng, inner_h, params.min_room_size, params.max_room_size);
    let max_x_off = inner_w.saturating_sub(w);
    let max_y_off = inner_h.saturating_sub(h);
    let dx = if max_x_off > 0 { rng.gen_range(0..=max_x_off) } else { 0 };
    let dy = if max_y_off > 0 { rng.gen_range(0..=max_y_off) } else { 0 };
    Room {
        x: rect.x + 1 + dx,
        y: rect.y + 1 + dy,
        w,
        h,
    }
}

fn clamp_size<R: Rng + ?Sized>(rng: &mut R, available: u32, min: u32, max: u32) -> u32 {
    let cap = available.min(max).max(min.min(available));
    if cap <= min { cap } else { rng.gen_range(min..=cap) }
}

fn carve_room(room: &Room, tiles: &mut [Tile], width: u32) {
    for ty in room.y..room.y + room.h {
        for tx in room.x..room.x + room.w {
            let idx = (ty * width + tx) as usize;
            if idx < tiles.len() {
                tiles[idx] = Tile::Floor;
            }
        }
    }
}

fn carve_corridor(a: (u32, u32), b: (u32, u32), tiles: &mut [Tile], width: u32) {
    let (ax, ay) = a;
    let (bx, by) = b;
    // L-corridor: horizontal from A's y to B's x, then vertical to B's y.
    for x in ax.min(bx)..=ax.max(bx) {
        let idx = (ay * width + x) as usize;
        if idx < tiles.len() {
            tiles[idx] = Tile::Floor;
        }
    }
    for y in ay.min(by)..=ay.max(by) {
        let idx = (y * width + bx) as usize;
        if idx < tiles.len() {
            tiles[idx] = Tile::Floor;
        }
    }
}

/// Parameters for [`generate_arena`] — the open debug/test room.
#[derive(Debug, Clone, Copy)]
pub struct ArenaParams {
    pub width: u32,
    pub height: u32,
    /// When true, scatter a regular grid of 2×2 wall pillars for the enemy
    /// pathfinder to route around. Spacing leaves wide corridors so the floor
    /// stays fully connected.
    pub pillars: bool,
    pub seed: u64,
}

impl Default for ArenaParams {
    fn default() -> Self {
        Self {
            width: 48,
            height: 36,
            pillars: true,
            seed: 1,
        }
    }
}

/// Generate a single big rectangular room: solid border wall, floor interior,
/// player spawn at the center, optional pillar grid. Unlike [`generate_bsp`]
/// the layout is fully determined by the parameters — the `seed` only labels
/// the map and seeds downstream gameplay RNG; same params → identical map.
///
/// Purpose-built for the debug level: open sightlines for weapon/TTK tuning,
/// and (with `pillars`) non-trivial geometry to watch the flow-field router
/// steer a swarm around. A connectivity test guarantees every floor tile is
/// reachable from spawn, same invariant the BSP maps hold.
pub fn generate_arena(params: &ArenaParams) -> Map {
    // Floor below ~7 leaves no room for a border + interior; clamp so callers
    // can't produce a degenerate (all-wall) map.
    let w = params.width.max(7);
    let h = params.height.max(7);
    let mut tiles = vec![Tile::Floor; (w * h) as usize];
    let idx = |x: u32, y: u32| (y * w + x) as usize;

    // Border walls.
    for x in 0..w {
        tiles[idx(x, 0)] = Tile::Wall;
        tiles[idx(x, h - 1)] = Tile::Wall;
    }
    for y in 0..h {
        tiles[idx(0, y)] = Tile::Wall;
        tiles[idx(w - 1, y)] = Tile::Wall;
    }

    if params.pillars {
        // 2×2 pillars on a 6-tile grid → 4-wide corridors between them, and a
        // floor ring inside the border (the grid starts at offset 3), so the
        // perimeter is always a connected loop regardless of interior layout.
        const SPACING: u32 = 6;
        let mut py = 3;
        while py + 1 < h - 2 {
            let mut px = 3;
            while px + 1 < w - 2 {
                for dy in 0..2 {
                    for dx in 0..2 {
                        tiles[idx(px + dx, py + dy)] = Tile::Wall;
                    }
                }
                px += SPACING;
            }
            py += SPACING;
        }
    }

    // Spawn at the center, with its 3×3 neighborhood cleared to floor in case a
    // pillar landed on it.
    let (cx, cy) = (w / 2, h / 2);
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            let x = cx as i32 + dx;
            let y = cy as i32 + dy;
            if x > 0 && y > 0 && (x as u32) < w - 1 && (y as u32) < h - 1 {
                tiles[idx(x as u32, y as u32)] = Tile::Floor;
            }
        }
    }

    Map {
        width: w,
        height: h,
        tiles,
        rooms: vec![Room {
            x: 1,
            y: 1,
            w: w - 2,
            h: h - 2,
        }],
        seed: params.seed,
        player_spawn: (cx, cy),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(seed: u64) -> MapParams {
        MapParams {
            seed,
            ..Default::default()
        }
    }

    #[test]
    fn same_seed_produces_identical_map() {
        let a = generate_bsp(&params(42));
        let b = generate_bsp(&params(42));
        assert_eq!(a.tiles, b.tiles);
        assert_eq!(a.rooms.len(), b.rooms.len());
        for (ra, rb) in a.rooms.iter().zip(&b.rooms) {
            assert_eq!((ra.x, ra.y, ra.w, ra.h), (rb.x, rb.y, rb.w, rb.h));
        }
        assert_eq!(a.player_spawn, b.player_spawn);
    }

    #[test]
    fn different_seeds_produce_different_maps() {
        let a = generate_bsp(&params(1));
        let b = generate_bsp(&params(2));
        assert_ne!(a.tiles, b.tiles, "two seeds shouldn't collide on the same map");
    }

    #[test]
    fn map_has_at_least_one_room() {
        let m = generate_bsp(&params(7));
        assert!(!m.rooms.is_empty(), "BSP must produce at least one room");
    }

    #[test]
    fn player_spawn_is_on_floor() {
        let m = generate_bsp(&params(123));
        let (sx, sy) = m.player_spawn;
        assert_eq!(
            m.tile_at(sx, sy),
            Tile::Floor,
            "spawn at ({sx},{sy}) must be on a Floor tile"
        );
    }

    #[test]
    fn tile_at_returns_wall_out_of_bounds() {
        let m = generate_bsp(&params(0));
        assert_eq!(m.tile_at(m.width, 0), Tile::Wall);
        assert_eq!(m.tile_at(0, m.height), Tile::Wall);
        assert_eq!(m.tile_at(u32::MAX, u32::MAX), Tile::Wall);
    }

    // ---- arena ----

    #[test]
    fn arena_spawn_is_floor_and_deterministic() {
        let a = generate_arena(&ArenaParams::default());
        let b = generate_arena(&ArenaParams::default());
        assert_eq!(a.tiles, b.tiles, "same params → identical arena");
        let (sx, sy) = a.player_spawn;
        assert_eq!(a.tile_at(sx, sy), Tile::Floor, "spawn must be floor");
    }

    #[test]
    fn arena_floor_is_fully_connected() {
        // The load-bearing pathfinding invariant: with or without pillars,
        // every floor tile must be reachable from spawn (no walled-off pockets
        // for a swarm to get stuck behind).
        for pillars in [false, true] {
            let m = generate_arena(&ArenaParams {
                pillars,
                ..Default::default()
            });
            let flow = FlowField::compute(&m, m.player_spawn);
            for y in 0..m.height {
                for x in 0..m.width {
                    if m.tile_at(x, y) == Tile::Floor {
                        assert_ne!(
                            flow.distance_at(x, y),
                            UNREACHABLE,
                            "floor ({x},{y}) unreachable from spawn (pillars={pillars})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn arena_border_is_wall() {
        let m = generate_arena(&ArenaParams::default());
        for x in 0..m.width {
            assert_eq!(m.tile_at(x, 0), Tile::Wall);
            assert_eq!(m.tile_at(x, m.height - 1), Tile::Wall);
        }
        for y in 0..m.height {
            assert_eq!(m.tile_at(0, y), Tile::Wall);
            assert_eq!(m.tile_at(m.width - 1, y), Tile::Wall);
        }
    }

    #[test]
    fn ascii_renders_one_char_per_tile_plus_newlines() {
        let m = generate_bsp(&params(3));
        let s = m.render_ascii();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), m.height as usize);
        for line in &lines {
            assert_eq!(line.chars().count(), m.width as usize);
        }
        // Player spawn shows as '@' exactly once.
        let at_count = s.chars().filter(|&c| c == '@').count();
        assert_eq!(at_count, 1);
    }

    #[test]
    fn rooms_fit_inside_map_bounds() {
        let m = generate_bsp(&params(99));
        for r in &m.rooms {
            assert!(r.x + r.w <= m.width, "room overruns width: {r:?}");
            assert!(r.y + r.h <= m.height, "room overruns height: {r:?}");
            assert!(r.w > 0 && r.h > 0, "degenerate room: {r:?}");
        }
    }
}
