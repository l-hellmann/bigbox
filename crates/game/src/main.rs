//! macroquad shell: window, input collection, render loop. The state
//! mutations all live in `h2b_game::World`; this file is the runtime
//! adapter — keystrokes → `Command`s, world tick, draw.

use h2b_game::{Command, Player, World};
use h2b_procgen::{Map, MapParams, Tile, generate_bsp};
use macroquad::prelude::*;

/// Pixels per tile. The default map is 80×40, so 16px = 1280×640 — fits a
/// reasonable window without scrolling for the first cut.
const TILE_SIZE: f32 = 16.0;
/// Player movement speed in tiles per second.
const PLAYER_SPEED: f32 = 6.0;
/// Map seed — wired via env var for now (`H2B_SEED=N`), defaults to 42.
fn map_seed() -> u64 {
    std::env::var("H2B_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(42)
}

fn window_conf() -> Conf {
    Conf {
        window_title: "head2box".into(),
        window_width: 1280,
        window_height: 680,
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let seed = map_seed();
    let map = generate_bsp(&MapParams {
        seed,
        ..Default::default()
    });
    let mut world = World::new(map);

    loop {
        let dt = get_frame_time();

        if is_key_pressed(KeyCode::Escape) {
            break;
        }
        for cmd in collect_input() {
            world.apply(cmd, dt, PLAYER_SPEED);
        }

        clear_background(BLACK);
        draw_map(&world.map);
        draw_player(&world.player);
        draw_hud(&world, seed);

        next_frame().await;
    }
}

fn collect_input() -> Vec<Command> {
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
    if dx == 0.0 && dy == 0.0 {
        return Vec::new();
    }
    let len = (dx * dx + dy * dy).sqrt();
    vec![Command::Move {
        dx: dx / len,
        dy: dy / len,
    }]
}

fn draw_map(map: &Map) {
    let wall = Color::new(0.18, 0.18, 0.20, 1.0);
    let floor = Color::new(0.08, 0.08, 0.10, 1.0);
    for ty in 0..map.height {
        for tx in 0..map.width {
            let color = match map.tile_at(tx, ty) {
                Tile::Wall => wall,
                Tile::Floor => floor,
            };
            draw_rectangle(
                tx as f32 * TILE_SIZE,
                ty as f32 * TILE_SIZE,
                TILE_SIZE,
                TILE_SIZE,
                color,
            );
        }
    }
}

fn draw_player(p: &Player) {
    let r = TILE_SIZE * 0.4;
    draw_circle(
        p.x * TILE_SIZE,
        p.y * TILE_SIZE,
        r,
        Color::new(0.35, 0.85, 0.35, 1.0),
    );
}

fn draw_hud(world: &World, seed: u64) {
    let y = (world.map.height as f32 * TILE_SIZE) + 18.0;
    draw_text(
        &format!(
            "WASD / arrows — move    ESC — quit    seed: {seed}    pos: ({:.1}, {:.1})",
            world.player.x, world.player.y
        ),
        8.0,
        y,
        18.0,
        Color::new(0.75, 0.75, 0.75, 1.0),
    );
}
