//! macroquad shell: window, input collection, render loop. The state
//! mutations all live in `h2b_game::World`; this file is the runtime
//! adapter — keystrokes + mouse → `Command`s, world tick, draw.
//!
//! Rendering is **boxy 3D** (BoxHead-style): the world is a 2D plane in
//! gameplay terms, drawn in 3D space with extruded-cube walls and an angled
//! follow-camera. Coordinate convention: a gameplay tile position `(x, y)`
//! maps to world `(x, _, y)` — world **X = tile x**, world **Z = tile y**,
//! world **Y = up**. So gameplay coords need no translation; only height (Y)
//! is invented here at the render layer.

use h2b_game::{Command, Player, Projectile, World};
use h2b_procgen::{MapParams, Tile, generate_bsp};
use macroquad::prelude::*;

/// How tall wall cubes stand, in world units (= tiles). Tall enough to read
/// depth and occlude, short enough not to hide the play area from the
/// angled cam.
const WALL_HEIGHT: f32 = 1.6;
/// Camera offset above the player, in world units. With `CAMERA_BACK` this
/// sets the tilt: atan(HEIGHT / BACK) ≈ 56° — the classic BoxHead overhead
/// angle that still shows wall height.
const CAMERA_HEIGHT: f32 = 15.0;
/// Camera offset toward +Z (screen-down / "south") from the player. The cam
/// looks down-and-forward at the player from here.
const CAMERA_BACK: f32 = 10.0;
/// Don't draw wall cubes farther than this (tiles) from the player. The cam
/// only frames a small neighborhood; culling keeps the per-frame draw-call
/// count bounded regardless of map size.
const RENDER_RADIUS: f32 = 30.0;
/// Player cube half-extent (render only; collision uses point tests in core).
const PLAYER_HALF: f32 = 0.35;

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
        window_height: 720,
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

        // Build the follow-cam first: aim raycasting needs it to unproject
        // the cursor onto the ground plane.
        let camera = build_camera(&world.player);
        let aim_hit = ground_hit(&camera);
        let aim = aim_direction(&world.player, aim_hit);

        for cmd in collect_input(aim) {
            world.apply(cmd, dt);
        }
        world.tick(dt);

        // ---- 3D pass ----
        clear_background(Color::new(0.02, 0.02, 0.03, 1.0));
        set_camera(&camera);
        draw_scene(&world, aim_hit);

        // ---- 2D overlay pass ----
        set_default_camera();
        draw_hud(&world, seed);

        next_frame().await;
    }
}

/// Angled follow-camera fixed in orientation (it does not rotate with the
/// player — only its position tracks). Mounted above and toward +Z so the
/// player reads as roughly centered with `−Z` ("north") going up-screen,
/// matching WASD's `W → −dy` intuition.
fn build_camera(p: &Player) -> Camera3D {
    let center = vec3(p.x, 0.0, p.y);
    let eye = center + vec3(0.0, CAMERA_HEIGHT, CAMERA_BACK);
    Camera3D {
        position: eye,
        target: center,
        up: vec3(0.0, 1.0, 0.0),
        ..Default::default()
    }
}

/// Cast a ray from the camera through the mouse cursor and intersect the
/// ground plane (`Y = 0`). Returns the world-space hit point, or `None` if
/// the ray is parallel to / pointing away from the ground (degenerate; the
/// angled cam makes it practically impossible, but guard anyway).
fn ground_hit(cam: &Camera3D) -> Option<Vec3> {
    let (mx, my) = mouse_position();
    let (sw, sh) = (screen_width(), screen_height());

    // Pixel → normalized device coords (Y flipped: screen Y grows downward).
    let ndc_x = (mx / sw) * 2.0 - 1.0;
    let ndc_y = 1.0 - (my / sh) * 2.0;

    // Camera basis.
    let forward = (cam.target - cam.position).normalize();
    let right = forward.cross(cam.up).normalize();
    let up = right.cross(forward);

    // Pinhole reconstruction: at unit distance the image half-height is
    // tan(fovy/2), half-width that × aspect.
    let tan_half = (cam.fovy * 0.5).tan();
    let aspect = sw / sh;
    let dir =
        (forward + right * (ndc_x * tan_half * aspect) + up * (ndc_y * tan_half)).normalize();

    if dir.y.abs() < 1e-6 {
        return None;
    }
    let t = -cam.position.y / dir.y;
    if t <= 0.0 {
        return None;
    }
    Some(cam.position + dir * t)
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

fn collect_input(aim: Option<(f32, f32)>) -> Vec<Command> {
    let mut cmds = Vec::new();

    // Movement. dy is along world Z, so W (−dy) moves "up"/north on screen.
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

/// Draw the full 3D scene: ground plane, extruded wall cubes (distance
/// culled), player, projectiles, and the aim line/marker.
fn draw_scene(world: &World, aim_hit: Option<Vec3>) {
    let map = &world.map;

    // Floor: one big plane under everything. `size` is the half-extent, so
    // it spans the full map when centered at the middle.
    let fw = map.width as f32;
    let fh = map.height as f32;
    draw_plane(
        vec3(fw * 0.5, 0.0, fh * 0.5),
        vec2(fw * 0.5, fh * 0.5),
        None,
        Color::new(0.07, 0.07, 0.09, 1.0),
    );

    // Walls: extruded cubes, only near the player. Cube + darker wireframe
    // gives the crisp boxy edge BoxHead reads by.
    let wall = Color::new(0.20, 0.20, 0.24, 1.0);
    let edge = Color::new(0.05, 0.05, 0.07, 1.0);
    let (px, py) = (world.player.x, world.player.y);
    let r2 = RENDER_RADIUS * RENDER_RADIUS;
    for ty in 0..map.height {
        for tx in 0..map.width {
            if !matches!(map.tile_at(tx, ty), Tile::Wall) {
                continue;
            }
            let cx = tx as f32 + 0.5;
            let cz = ty as f32 + 0.5;
            if (cx - px).powi(2) + (cz - py).powi(2) > r2 {
                continue;
            }
            let center = vec3(cx, WALL_HEIGHT * 0.5, cz);
            let size = vec3(1.0, WALL_HEIGHT, 1.0);
            draw_cube(center, size, None, wall);
            draw_cube_wires(center, size, edge);
        }
    }

    // Player: a green cube sitting on the floor.
    let pc = vec3(px, PLAYER_HALF, py);
    let psize = vec3(PLAYER_HALF * 2.0, PLAYER_HALF * 2.0, PLAYER_HALF * 2.0);
    draw_cube(pc, psize, None, Color::new(0.35, 0.85, 0.35, 1.0));
    draw_cube_wires(pc, psize, Color::new(0.12, 0.40, 0.12, 1.0));

    // Projectiles: small glowing cubes floating at mid-height.
    for p in &world.projectiles {
        draw_projectile(p);
    }

    // Aim: a line from the player along the ground to the cursor hit, plus a
    // small marker cube at the hit — the 3D replacement for the 2D crosshair.
    if let Some(hit) = aim_hit {
        let from = vec3(px, 0.25, py);
        let to = vec3(hit.x, 0.05, hit.z);
        draw_line_3d(from, to, Color::new(0.6, 0.6, 0.65, 0.5));
        draw_cube_wires(
            vec3(hit.x, 0.1, hit.z),
            vec3(0.3, 0.2, 0.3),
            Color::new(0.9, 0.3, 0.3, 0.9),
        );
    }
}

fn draw_projectile(p: &Projectile) {
    let c = vec3(p.x, 0.5, p.y);
    let s = vec3(0.22, 0.22, 0.22);
    draw_cube(c, s, None, Color::new(1.0, 0.9, 0.4, 1.0));
}

fn draw_hud(world: &World, seed: u64) {
    draw_text(
        &format!(
            "WASD/arrows move  |  LMB or Space shoot  |  ESC quit  \
             |  seed: {seed}  pos: ({:.1}, {:.1})  shots in flight: {}",
            world.player.x,
            world.player.y,
            world.projectiles.len(),
        ),
        12.0,
        screen_height() - 16.0,
        20.0,
        Color::new(0.75, 0.75, 0.75, 1.0),
    );
}