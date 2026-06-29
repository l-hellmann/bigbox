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
//!
//! Content (enemy roster, base items, affixes) is loaded from RON on the
//! **native** target via the filesystem. The wasm target can't read files at
//! runtime — that build will `include_str!` the content instead, swapped in
//! when wasm packaging lands (a separate ⏳ scope item).

use h2b_core::progression::level_for_total_xp;
use h2b_core::Rarity;
use h2b_game::{Command, Content, EnemyInstance, LootDrop, Player, Projectile, World};
use h2b_procgen::{ArenaParams, Map, MapParams, Tile, generate_arena, generate_bsp};
use macroquad::prelude::*;

#[cfg(feature = "debug")]
mod debug;

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

/// Content is **embedded at build time** via `include_str!`, not read from
/// disk. That's the only thing that works on the wasm target (no filesystem)
/// and keeps a shipped native binary self-contained — no content directory to
/// ship alongside it. A parse failure here means malformed RON in the tree,
/// i.e. a build-time content bug, so panicking is correct. (Dev hot-reload, if
/// we want it, layers back as a debug-only disk override behind a `cfg`.)
fn load_content() -> Content {
    Content {
        enemies: h2b_content::parse_enemies("enemies.ron", include_str!("../../content/data/enemies.ron"))
            .expect("embedded enemies.ron"),
        bases: h2b_content::parse_bases("bases.ron", include_str!("../../content/data/bases.ron"))
            .expect("embedded bases.ron"),
        affixes: h2b_content::parse_affixes("affixes.ron", include_str!("../../content/data/affixes.ron"))
            .expect("embedded affixes.ron"),
    }
}

/// Pick the starting map from the `H2B_LEVEL` env var: `arena` (open debug room
/// with pillars), `arena-empty` (no pillars), or anything else / unset → the
/// regular BSP dungeon. The debug overlay can also swap maps live at runtime.
fn starting_map(seed: u64) -> Map {
    match std::env::var("H2B_LEVEL").as_deref() {
        Ok("arena") => generate_arena(&ArenaParams {
            seed,
            ..Default::default()
        }),
        Ok("arena-empty") => generate_arena(&ArenaParams {
            seed,
            pillars: false,
            ..Default::default()
        }),
        _ => generate_bsp(&MapParams {
            seed,
            ..Default::default()
        }),
    }
}

fn new_world(seed: u64) -> World {
    World::new(starting_map(seed))
}

#[macroquad::main(window_conf)]
async fn main() {
    let seed = map_seed();
    let content = load_content();
    let mut world = new_world(seed);

    #[cfg(feature = "debug")]
    let mut dbg = debug::DebugUi::new();

    loop {
        let dt = get_frame_time();

        if is_key_pressed(KeyCode::Escape) {
            break;
        }
        // Restart from the dead screen.
        if world.game_over && is_key_pressed(KeyCode::R) {
            world = new_world(seed);
        }

        // Build the follow-cam first: aim raycasting needs it to unproject
        // the cursor onto the ground plane.
        let camera = build_camera(&world.player);
        let aim_hit = ground_hit(&camera);
        let aim = aim_direction(&world.player, aim_hit);

        // Debug overlay runs before input so its spawns/tunable edits apply to
        // this frame's tick, and so it can swallow clicks that land on the
        // panel (`block_fire`).
        #[cfg(feature = "debug")]
        let block_fire = {
            dbg.handle_toggle();
            dbg.run(&mut world, &content, aim_hit.map(|h| (h.x, h.z)))
        };
        #[cfg(not(feature = "debug"))]
        let block_fire = false;

        for cmd in collect_input(aim) {
            if block_fire && matches!(cmd, Command::Fire { .. }) {
                continue;
            }
            world.apply(cmd, dt);
        }
        world.tick(dt, &content);

        // ---- 3D pass ----
        clear_background(Color::new(0.02, 0.02, 0.03, 1.0));
        set_camera(&camera);
        draw_scene(&world, &content, aim_hit);
        #[cfg(feature = "debug")]
        if dbg.show_flow() {
            draw_flow_field(&world);
        }

        // ---- 2D overlay pass ----
        set_default_camera();
        draw_hud(&world);

        // ---- debug overlay (on top of everything) ----
        #[cfg(feature = "debug")]
        dbg.draw();

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
    if fire_held && let Some((adx, ady)) = aim {
        cmds.push(Command::Fire { dx: adx, dy: ady });
    }

    cmds
}

/// Draw the full 3D scene: ground plane, extruded wall cubes (distance
/// culled), enemies, ground drops (with loot beams), player, projectiles,
/// and the aim line/marker.
fn draw_scene(world: &World, content: &Content, aim_hit: Option<Vec3>) {
    let map = &world.map;
    let (px, py) = (world.player.x, world.player.y);

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

    // Drops: a small cube plus a vertical rarity-colored loot beam so they're
    // spottable across the room.
    for d in &world.drops {
        draw_drop(d);
    }

    // Enemies: per-archetype colored cubes sitting on the floor.
    for e in &world.enemies {
        draw_enemy(e, content);
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

/// Debug pathing viz: a short cyan arrow on each reachable floor tile pointing
/// to the flow field's next step toward the player, and a yellow pad on the
/// goal tile. Reads the same `FlowField` the enemies steer by, so it shows
/// exactly how a swarm will route — around pillars, through doorways, etc.
#[cfg(feature = "debug")]
fn draw_flow_field(world: &World) {
    use h2b_procgen::UNREACHABLE;
    let flow = world.flow();
    let map = &world.map;
    let (px, py) = (world.player.x, world.player.y);
    let r2 = RENDER_RADIUS * RENDER_RADIUS;
    for ty in 0..map.height {
        for tx in 0..map.width {
            if !matches!(map.tile_at(tx, ty), Tile::Floor) {
                continue;
            }
            let cx = tx as f32 + 0.5;
            let cz = ty as f32 + 0.5;
            if (cx - px).powi(2) + (cz - py).powi(2) > r2 {
                continue;
            }
            if flow.distance_at(tx, ty) == UNREACHABLE {
                continue;
            }
            match flow.next_step_from(tx, ty) {
                Some((nx, ny)) => {
                    let dx = (nx as f32 + 0.5) - cx;
                    let dz = (ny as f32 + 0.5) - cz;
                    let from = vec3(cx, 0.06, cz);
                    let to = vec3(cx + dx * 0.5, 0.06, cz + dz * 0.5);
                    draw_line_3d(from, to, Color::new(0.30, 0.80, 1.00, 0.7));
                    draw_cube(to, vec3(0.09, 0.02, 0.09), None, Color::new(0.40, 0.95, 1.00, 0.9));
                }
                None => {
                    draw_cube(
                        vec3(cx, 0.06, cz),
                        vec3(0.18, 0.02, 0.18),
                        None,
                        Color::new(1.00, 1.00, 0.40, 0.85),
                    );
                }
            }
        }
    }
}

fn draw_enemy(e: &EnemyInstance, content: &Content) {
    let id = content
        .enemies
        .get(e.archetype)
        .map(|a| a.id.as_str())
        .unwrap_or("");
    let (color, half) = enemy_visual(id);
    let center = vec3(e.x, half, e.y);
    let size = vec3(half * 2.0, half * 2.0, half * 2.0);
    draw_cube(center, size, None, color);
    draw_cube_wires(center, size, Color::new(0.05, 0.02, 0.02, 1.0));
}

/// (color, half-extent) per enemy archetype. Bigger, redder reads as tankier.
fn enemy_visual(id: &str) -> (Color, f32) {
    match id {
        "swarm_rusher" => (Color::new(0.80, 0.50, 0.30, 1.0), 0.28),
        "fast_zombie" => (Color::new(0.90, 0.70, 0.20, 1.0), 0.30),
        "spitter" => (Color::new(0.50, 0.80, 0.40, 1.0), 0.34),
        "basic_zombie" => (Color::new(0.75, 0.25, 0.25, 1.0), 0.38),
        "fat_zombie" => (Color::new(0.55, 0.20, 0.50, 1.0), 0.58),
        "patient_zero" => (Color::new(0.95, 0.10, 0.10, 1.0), 0.85),
        _ => (Color::new(0.75, 0.25, 0.25, 1.0), 0.38),
    }
}

fn draw_drop(d: &LootDrop) {
    let color = rarity_color(d.item.rarity);
    draw_cube(vec3(d.x, 0.18, d.y), vec3(0.3, 0.3, 0.3), None, color);
    // Loot beam — a vertical line, brighter rarities literally stand taller.
    let beam_top = 1.0 + d.item.rarity.index() as f32 * 0.6;
    draw_line_3d(vec3(d.x, 0.0, d.y), vec3(d.x, beam_top, d.y), color);
}

fn rarity_color(r: Rarity) -> Color {
    match r {
        Rarity::Basic => Color::new(0.70, 0.70, 0.70, 1.0),
        Rarity::Common => Color::new(0.45, 0.70, 1.00, 1.0),
        Rarity::Rare => Color::new(1.00, 0.85, 0.20, 1.0),
        Rarity::Epic => Color::new(0.70, 0.30, 1.00, 1.0),
        Rarity::Legendary => Color::new(1.00, 0.50, 0.10, 1.0),
    }
}

fn draw_projectile(p: &Projectile) {
    let c = vec3(p.x, 0.5, p.y);
    let s = vec3(0.22, 0.22, 0.22);
    draw_cube(c, s, None, Color::new(1.0, 0.9, 0.4, 1.0));
}

/// 2D overlay: health bar, run stats, and the dead screen.
fn draw_hud(world: &World) {
    // Health bar, top-left.
    let (bx, by, bw, bh) = (12.0, 12.0, 260.0, 22.0);
    draw_rectangle(bx, by, bw, bh, Color::new(0.15, 0.05, 0.05, 0.9));
    let frac = (world.player.current_life / world.player.max_life).clamp(0.0, 1.0);
    draw_rectangle(bx, by, bw * frac, bh, Color::new(0.85, 0.20, 0.20, 1.0));
    draw_rectangle_lines(bx, by, bw, bh, 2.0, Color::new(0.0, 0.0, 0.0, 1.0));
    draw_text(
        &format!(
            "{:.0} / {:.0}",
            world.player.current_life, world.player.max_life
        ),
        bx + 8.0,
        by + 17.0,
        20.0,
        WHITE,
    );

    // Run stats line.
    let level = level_for_total_xp(world.xp);
    let stats = format!(
        "Lv {level}   kills {}   xp {}   enemies {}   loot {}",
        world.kills,
        world.xp,
        world.enemies.len(),
        world.inventory.len(),
    );
    draw_text(&stats, 12.0, 56.0, 22.0, Color::new(0.85, 0.85, 0.85, 1.0));

    if let Some(last) = &world.last_pickup {
        draw_text(
            &format!("picked up: {last}"),
            12.0,
            78.0,
            18.0,
            Color::new(0.7, 0.85, 0.7, 1.0),
        );
    }

    // Controls hint, bottom.
    draw_text(
        "WASD move  |  LMB / Space shoot  |  ESC quit",
        12.0,
        screen_height() - 16.0,
        18.0,
        Color::new(0.55, 0.55, 0.55, 1.0),
    );

    if world.game_over {
        let cx = screen_width() * 0.5;
        let cy = screen_height() * 0.5;
        let msg = "YOU DIED";
        let d = measure_text(msg, None, 64, 1.0);
        draw_text(msg, cx - d.width * 0.5, cy, 64.0, Color::new(0.9, 0.1, 0.1, 1.0));
        let sub = "press R to restart";
        let d2 = measure_text(sub, None, 28, 1.0);
        draw_text(sub, cx - d2.width * 0.5, cy + 40.0, 28.0, WHITE);
    }
}
