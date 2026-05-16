//! macroquad shell: window, input collection, render loop. The state
//! mutations all live in `h2b_game::World`; this file is the runtime
//! adapter — keystrokes + mouse → `Command`s, world tick, draw.

use h2b_game::{Command, Player, Projectile, World};
use h2b_procgen::{Map, MapParams, Tile, generate_bsp};
use macroquad::prelude::*;

/// Pixels per tile. The default map is 80×40, so 16px = 1280×640 — fits a
/// reasonable window without scrolling for the first cut.
const TILE_SIZE: f32 = 16.0;

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

        let aim = aim_vector(&world.player);
        for cmd in collect_input(aim) {
            world.apply(cmd, dt);
        }
        world.tick(dt);

        clear_background(BLACK);
        draw_map(&world.map);
        for p in &world.projectiles {
            draw_projectile(p);
        }
        draw_player(&world.player);
        draw_crosshair(aim, &world.player);
        draw_hud(&world, seed);

        next_frame().await;
    }
}

/// Mouse cursor (pixels) → normalized aim vector in tile-space relative to
/// the player. Returns `None` only at the degenerate point where the cursor
/// sits exactly on the player.
fn aim_vector(p: &Player) -> Option<(f32, f32)> {
    let (mx, my) = mouse_position();
    let dx = (mx / TILE_SIZE) - p.x;
    let dy = (my / TILE_SIZE) - p.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 {
        None
    } else {
        Some((dx / len, dy / len))
    }
}

fn collect_input(aim: Option<(f32, f32)>) -> Vec<Command> {
    let mut cmds = Vec::new();

    // Movement
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
        cmds.push(Command::Move {
            dx: dx / len,
            dy: dy / len,
        });
    }

    // Fire — continuous while LMB or space held. World enforces cooldown.
    let fire_held = is_mouse_button_down(MouseButton::Left) || is_key_down(KeyCode::Space);
    if fire_held {
        if let Some((adx, ady)) = aim {
            cmds.push(Command::Fire { dx: adx, dy: ady });
        }
    }

    cmds
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

fn draw_projectile(p: &Projectile) {
    let r = TILE_SIZE * 0.15;
    draw_circle(
        p.x * TILE_SIZE,
        p.y * TILE_SIZE,
        r,
        Color::new(1.0, 0.9, 0.4, 1.0),
    );
}

/// Faint line from the player along the aim direction so the player can see
/// where they're pointing without staring at the mouse cursor.
fn draw_crosshair(aim: Option<(f32, f32)>, p: &Player) {
    let Some((dx, dy)) = aim else { return };
    let len_tiles = 2.0;
    let start = (p.x * TILE_SIZE, p.y * TILE_SIZE);
    let end = (
        (p.x + dx * len_tiles) * TILE_SIZE,
        (p.y + dy * len_tiles) * TILE_SIZE,
    );
    draw_line(
        start.0,
        start.1,
        end.0,
        end.1,
        1.0,
        Color::new(0.6, 0.6, 0.65, 0.4),
    );
}

fn draw_hud(world: &World, seed: u64) {
    let y = (world.map.height as f32 * TILE_SIZE) + 18.0;
    draw_text(
        &format!(
            "WASD/arrows move  |  LMB or Space shoot  |  ESC quit  \
             |  seed: {seed}  pos: ({:.1}, {:.1})  shots in flight: {}",
            world.player.x,
            world.player.y,
            world.projectiles.len(),
        ),
        8.0,
        y,
        18.0,
        Color::new(0.75, 0.75, 0.75, 1.0),
    );
}
