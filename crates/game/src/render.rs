//! Boxy-3D rendering (BoxHead-style): the world is a 2D plane in gameplay
//! terms, drawn in 3D space with extruded-cube walls and an angled follow-cam.
//! Coordinate convention: a gameplay tile position `(x, y)` maps to world
//! `(x, _, y)` — world **X = tile x**, world **Z = tile y**, world **Y = up**.
//! So gameplay coords need no translation; only height (Y) is invented here.
//!
//! Pure consumer of `&World`: every function reads world/content state and
//! draws, mutating nothing. The debug-only viz (flow field, entity stat blocks)
//! is feature-gated to match its callers.

use h2b_core::progression::level_for_total_xp;
use h2b_core::Rarity;
use h2b_game::{Content, EnemyInstance, Explosion, FireProfile, LootDrop, Projectile, World};
use h2b_procgen::{Map, Tile};
use macroquad::prelude::*;

/// How tall wall cubes stand, in world units (= tiles). Tall enough to read
/// depth and occlude, short enough not to hide the play area from the
/// angled cam.
const WALL_HEIGHT: f32 = 1.6;
/// Don't draw wall cubes farther than this (tiles) from the player. The cam
/// only frames a small neighborhood; culling keeps the per-frame draw-call
/// count bounded regardless of map size.
const RENDER_RADIUS: f32 = 30.0;
/// Player cube half-extent (render only; collision uses point tests in core).
const PLAYER_HALF: f32 = 0.35;
/// The two floor-checker shades. Kept close so the grid reads as a subtle
/// movement reference, not a distracting chessboard.
const FLOOR_DARK: Color = Color::new(0.07, 0.07, 0.09, 1.0);
const FLOOR_LIGHT: Color = Color::new(0.11, 0.11, 0.14, 1.0);
/// Radians of walk-cycle phase per tile moved — how fast limbs swing relative to
/// travel. Higher = quicker steps. Phase is `(x + y) * WALK_FREQ`.
const WALK_FREQ: f32 = 6.0;

/// Procedurally-generated render assets (built once, reused every frame). Kept
/// separate from `&World` so the renderer stays a pure state consumer and the
/// textures live outside the headless library.
pub struct RenderAssets {
    /// A small tileable grain, sampled per floor tile and tinted per checker
    /// square — gives the floor grit without a flat-colour look.
    floor: Texture2D,
}

impl RenderAssets {
    /// Generate all textures. Call once after the GL context exists (i.e. inside
    /// the macroquad `main`, before the loop).
    pub fn load() -> Self {
        Self { floor: make_floor_texture() }
    }
}

/// Draw the full 3D scene: ground plane, extruded wall cubes (distance
/// culled), enemies, ground drops (with loot beams), player, projectiles,
/// and the aim line/marker.
pub fn draw_scene(world: &World, content: &Content, aim_hit: Option<Vec3>, assets: &RenderAssets) {
    let map = &world.map;
    let (px, py) = (world.player.x, world.player.y);

    // Floor: one big dark plane under everything, as a fallback for tiles beyond
    // the textured near-field. `size` is the half-extent, so it spans the full
    // map when centered at the middle.
    let fw = map.width as f32;
    let fh = map.height as f32;
    draw_plane(
        vec3(fw * 0.5, 0.0, fh * 0.5),
        vec2(fw * 0.5, fh * 0.5),
        None,
        FLOOR_DARK,
    );
    let r2 = RENDER_RADIUS * RENDER_RADIUS;

    // Checkered, textured floor near the player: each tile is a single plane
    // (lifted a hair off the base to avoid z-fight) carrying the grain texture,
    // tinted dark/light by `(tx+ty)` parity. The alternating grid + grain give a
    // static reference against which entity movement reads clearly.
    for ty in 0..map.height {
        for tx in 0..map.width {
            let cx = tx as f32 + 0.5;
            let cz = ty as f32 + 0.5;
            if (cx - px).powi(2) + (cz - py).powi(2) > r2 {
                continue;
            }
            let tint = if (tx + ty) % 2 == 0 { FLOOR_LIGHT } else { FLOOR_DARK };
            draw_plane(vec3(cx, 0.005, cz), vec2(0.5, 0.5), Some(&assets.floor), tint);
        }
    }

    // Walls: extruded cubes, only near the player. Cube + darker wireframe
    // gives the crisp boxy edge BoxHead reads by.
    let wall = Color::new(0.20, 0.20, 0.24, 1.0);
    let edge = Color::new(0.05, 0.05, 0.07, 1.0);
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

    // Props: sparse barrels / crates / rubble scattered deterministically from
    // the map seed — scene dressing, not collision geometry (render-only).
    draw_props(map, px, py, r2);

    // Drops: a small cube plus a vertical rarity-colored loot beam so they're
    // spottable across the room.
    for d in &world.drops {
        draw_drop(d);
    }

    // Enemies: per-archetype box-figures sitting on the floor, yawed to face
    // their actual movement heading (`EnemyInstance.facing`).
    for e in &world.enemies {
        draw_enemy(e, content);
    }

    // Player: a green box-figure holding a gun, facing the aim direction.
    // Facing derives from the cursor's ground hit; when there's no aim
    // (degenerate), it defaults to +Z ("north").
    let pyaw = aim_hit
        .map(|h| (h.x - px, h.z - py))
        .filter(|(dx, dz)| dx * dx + dz * dz > 1e-6)
        .map_or(0.0, |(dx, dz)| dx.atan2(dz));
    draw_box_figure(
        vec3(px, 0.0, py),
        pyaw,
        &FigureSpec {
            scale: PLAYER_HALF,
            width: 1.0,
            body: Color::new(0.32, 0.72, 0.38, 1.0),
            head: Color::new(0.40, 0.85, 0.46, 1.0),
            accent: Color::new(0.85, 1.00, 0.55, 1.0),
            // Walk bob from the player's own position so it steps as it moves.
            phase: (px + py) * WALK_FREQ,
            gun: true,
        },
    );

    // Projectiles: small glowing cubes floating at mid-height.
    for p in &world.projectiles {
        draw_projectile(p);
    }

    // Rocket blasts: expanding, fading rings on the ground.
    for e in &world.explosions {
        draw_explosion(e);
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
pub fn draw_flow_field(world: &World) {
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
            // Show the actual smooth steering direction enemies follow (with
            // the discrete saddle fallback), not the raw discrete next-step.
            match flow.steer_from(cx, cz).or_else(|| flow.next_step_dir(cx, cz)) {
                Some((dx, dz)) => {
                    let from = vec3(cx, 0.06, cz);
                    let to = vec3(cx + dx * 0.4, 0.06, cz + dz * 0.4);
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

/// Project a world point to screen pixels using the active 3D camera matrix,
/// or `None` if it's behind the camera. Lets the 2D pass anchor text labels to
/// 3D entities.
#[cfg(feature = "debug")]
fn world_to_screen(view_proj: &Mat4, p: Vec3) -> Option<(f32, f32)> {
    let clip = *view_proj * vec4(p.x, p.y, p.z, 1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    let sx = (ndc.x * 0.5 + 0.5) * screen_width();
    let sy = (1.0 - (ndc.y * 0.5 + 0.5)) * screen_height();
    Some((sx, sy))
}

/// Draw a centered, stacked block of text lines on a dark translucent box,
/// anchored so the block sits *above* `(sx, sy)` (the entity's head). The box is
/// clamped to the screen so a label near an edge (notably the player's) stays
/// fully visible rather than sliding off.
#[cfg(feature = "debug")]
fn draw_label(lines: &[String], sx: f32, sy: f32, color: Color) {
    const SIZE: f32 = 14.0;
    const LINE_H: f32 = 15.0;
    const PAD: f32 = 4.0;

    let max_w = lines
        .iter()
        .map(|l| measure_text(l, None, SIZE as u16, 1.0).width)
        .fold(0.0_f32, f32::max);
    let bw = max_w + PAD * 2.0;
    let bh = LINE_H * lines.len() as f32 + PAD * 2.0;

    // Anchor above the head, then clamp the whole box inside the screen.
    let bx = (sx - bw * 0.5).clamp(0.0, (screen_width() - bw).max(0.0));
    let by = (sy - bh - PAD).clamp(0.0, (screen_height() - bh).max(0.0));

    draw_rectangle(bx, by, bw, bh, Color::new(0.02, 0.02, 0.05, 0.6));

    // Lines, baseline-stacked, centered within the box.
    let mut y = by + PAD + SIZE * 0.8;
    for line in lines {
        let w = measure_text(line, None, SIZE as u16, 1.0).width;
        draw_text(line, bx + (bw - w) * 0.5, y, SIZE, color);
        y += LINE_H;
    }
}

/// Debug overlay: floating `Enemy` + `Combatant` stat blocks above each nearby
/// enemy, and a `PLAYER` block above the player. Toggled from the debug panel.
#[cfg(feature = "debug")]
pub fn draw_entity_stats(world: &World, content: &Content, camera: &Camera3D) {
    use h2b_procgen::UNREACHABLE;
    use macroquad::camera::Camera;

    /// Only label this many enemies — the nearest ones — to keep it readable.
    const MAX_LABELS: usize = 5;

    let view_proj = camera.matrix();
    let (px, py) = (world.player.x, world.player.y);

    // Rank enemies by distance and label only the closest few.
    let mut ranked: Vec<(f32, usize)> = world
        .enemies
        .iter()
        .enumerate()
        .map(|(i, e)| ((e.x - px).powi(2) + (e.y - py).powi(2), i))
        .collect();
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    ranked.truncate(MAX_LABELS);

    // Draw the selected labels in descending id order so the lowest id ends up
    // drawn last — i.e. on top — when boxes overlap. Lower id wins the layer.
    ranked.sort_by(|a, b| world.enemies[b.1].id.cmp(&world.enemies[a.1].id));

    for &(d2, i) in &ranked {
        let e = &world.enemies[i];
        let id = content
            .enemies
            .get(e.archetype)
            .map(|a| a.id.as_str())
            .unwrap_or("?");
        let (_, scale, _) = enemy_shape(id);
        let head = vec3(e.x, figure_top(scale) + 0.5, e.y);
        let Some((sx, sy)) = world_to_screen(&view_proj, head) else {
            continue;
        };
        let c = &e.combatant;
        let ilvl = content.enemies.get(e.archetype).map(|a| a.ilvl).unwrap_or(0);
        let flow_d = world.flow().distance_at(e.x as u32, e.y as u32);
        let flow_s = if flow_d == UNREACHABLE {
            "∞".to_string()
        } else {
            flow_d.to_string()
        };
        let state = if e.awake { "" } else { "  [idle]" };
        let lines = [
            format!("{id} #{}  il{ilvl}{state}", e.id),
            format!("hp {:.0}/{:.0}", c.current_life, c.max_life),
            format!("arm {:.0}  eva {:.0}", c.armor, c.evasion),
            format!("spd {:.1}  d {:.1}  flow {flow_s}", e.speed, d2.sqrt()),
        ];
        draw_label(&lines, sx, sy, Color::new(0.97, 0.85, 0.55, 1.0));
    }

    // Player block — always drawn, independent of the enemy `MAX_LABELS` cap,
    // and last so it sits on top of any overlapping enemy labels.
    let p = &world.player;
    let head = vec3(px, figure_top(PLAYER_HALF) + 0.6, py);
    if let Some((sx, sy)) = world_to_screen(&view_proj, head) {
        let lines = [
            "PLAYER".to_string(),
            format!("hp {:.0}/{:.0}", p.current_life, p.max_life),
            format!("pos {:.1}, {:.1}  cd {:.2}", p.x, p.y, world.player_fire_cooldown),
            format!("enemies {}  proj {}", world.enemies.len(), world.projectiles.len()),
        ];
        draw_label(&lines, sx, sy, Color::new(0.55, 0.97, 0.62, 1.0));
    }
}

fn draw_enemy(e: &EnemyInstance, content: &Content) {
    let id = content
        .enemies
        .get(e.archetype)
        .map(|a| a.id.as_str())
        .unwrap_or("");
    let (color, scale, width) = enemy_shape(id);
    // Yaw to the enemy's actual movement heading (kept steady while stopped).
    // The walk phase is driven by the enemy's position so limbs step as it
    // actually moves and freeze when it holds — the movement reference the
    // checkered floor started.
    draw_box_figure(
        vec3(e.x, 0.0, e.y),
        e.facing,
        &FigureSpec {
            scale,
            width,
            body: color,
            head: lighten(color, 0.12),
            accent: Color::new(0.90, 0.96, 0.45, 1.0), // sickly zombie eyes
            phase: (e.x + e.y) * WALK_FREQ,
            gun: false,
        },
    );
    // Health bar drawn outside the yaw so it stays world-aligned (a rotating
    // bar under the fixed cam would be unreadable).
    draw_health_bar(e, scale);
}

/// A boxy humanoid ("box-person") built from stacked primitives — the BoxHead
/// silhouette. All measures scale off `scale` (≈ the old cube half-extent) so
/// the footprint matches the previous cubes; `width` fattens the torso/arms
/// (tanks read wide). Feet sit at `y = 0`.
struct FigureSpec {
    scale: f32,
    width: f32,
    body: Color,
    head: Color,
    accent: Color,
    /// Walk-cycle phase (radians); limbs swing by `sin(phase)`, so a figure
    /// whose phase tracks its position steps as it moves and stills when parked.
    phase: f32,
    /// Draw a forward gun in the right hand (the player).
    gun: bool,
}

/// Draw a [`FigureSpec`] at ground point `ground` (feet), yawed `yaw` about the
/// vertical so it faces its heading (front = local +Z). Everything is drawn
/// under one pushed model matrix; callers add world-aligned overlays (health
/// bars, labels) *outside* so they don't rotate with the body.
fn draw_box_figure(ground: Vec3, yaw: f32, f: &FigureSpec) {
    let s = f.scale;
    let w = f.width;
    let edge = Color::new(0.04, 0.03, 0.03, 1.0);
    let swing = f.phase.sin();

    draw_yawed(ground, yaw, || {
        // Legs — shuffle fore/aft in antiphase (a cheap two-step).
        let leg = vec3(0.42 * s, 0.75 * s, 0.5 * s);
        let leg_y = 0.375 * s;
        let leg_dz = swing * 0.18 * s;
        for (side, dz) in [(-1.0_f32, leg_dz), (1.0, -leg_dz)] {
            part(vec3(side * 0.30 * s, leg_y, dz), leg, f.body, edge);
        }

        // Torso.
        let torso = vec3(1.0 * s * w, 0.9 * s, 0.62 * s);
        let torso_y = 0.75 * s + 0.45 * s;
        part(vec3(0.0, torso_y, 0.0), torso, f.body, edge);

        // Arms — swing opposite the legs.
        let arm = vec3(0.28 * s, 0.82 * s, 0.4 * s);
        let arm_x = 0.5 * s * w + 0.2 * s;
        for (side, dz) in [(-1.0_f32, -swing * 0.2 * s), (1.0, swing * 0.2 * s)] {
            part(vec3(side * arm_x, torso_y - 0.02 * s, dz), arm, f.body, edge);
        }

        // Head + face (eyes on the +Z front, so the accent shows the heading).
        let head_sz = vec3(0.64 * s, 0.6 * s, 0.62 * s);
        let head_y = 0.75 * s + 0.9 * s + 0.3 * s;
        part(vec3(0.0, head_y, 0.0), head_sz, f.head, edge);
        let eye = vec3(0.14 * s, 0.14 * s, 0.06 * s);
        let eye_z = 0.31 * s + 0.02;
        for side in [-1.0_f32, 1.0] {
            draw_cube(vec3(side * 0.16 * s, head_y + 0.06 * s, eye_z), eye, None, f.accent);
        }

        // Gun: a dark barrel jutting forward from the right hand.
        if f.gun {
            let barrel = vec3(0.16 * s, 0.16 * s, 0.9 * s);
            let gun_col = Color::new(0.12, 0.12, 0.14, 1.0);
            part(
                vec3(arm_x, torso_y - 0.05 * s, 0.4 * s),
                barrel,
                gun_col,
                edge,
            );
        }
    });
}

/// One solid box part with its wire edge, drawn at the current model transform.
fn part(center: Vec3, size: Vec3, fill: Color, edge: Color) {
    draw_cube(center, size, None, fill);
    draw_cube_wires(center, size, edge);
}

/// Run `draw` with a translate+yaw model matrix pushed (macroquad's `draw_cube`
/// is axis-aligned). Acquire → push → drop the GL handle before drawing, since
/// `draw_cube` re-acquires it internally and we must not hold it across.
fn draw_yawed(center: Vec3, yaw: f32, draw: impl FnOnce()) {
    let m = Mat4::from_translation(center) * Mat4::from_rotation_y(yaw);
    unsafe { get_internal_gl() }.quad_gl.push_model_matrix(m);
    draw();
    unsafe { get_internal_gl() }.quad_gl.pop_model_matrix();
}

/// Lighten a colour toward white by `amt` (0..1) — used for the head vs body.
fn lighten(c: Color, amt: f32) -> Color {
    Color::new(
        c.r + (1.0 - c.r) * amt,
        c.g + (1.0 - c.g) * amt,
        c.b + (1.0 - c.b) * amt,
        c.a,
    )
}

/// Scatter sparse decorative props on floor tiles, chosen deterministically from
/// `(tile, map.seed)` — so the same map always dresses identically, with no
/// stored state and no per-frame RNG. Distance-culled like the walls. Purely
/// visual: props have no collision (a future pass could promote crates/barrels
/// to real cover by blocking those tiles in `can_stand`).
fn draw_props(map: &Map, px: f32, py: f32, r2: f32) {
    for ty in 0..map.height {
        for tx in 0..map.width {
            if !matches!(map.tile_at(tx, ty), Tile::Floor) {
                continue;
            }
            // Keep the spawn tile clear so the player never starts inside a prop.
            if (tx, ty) == map.player_spawn {
                continue;
            }
            let cx = tx as f32 + 0.5;
            let cz = ty as f32 + 0.5;
            if (cx - px).powi(2) + (cz - py).powi(2) > r2 {
                continue;
            }
            let h = tile_hash(tx, ty, map.seed);
            // ~1 in 17 floor tiles gets a prop.
            if h % 17 != 0 {
                continue;
            }
            match (h >> 8) % 3 {
                0 => draw_barrel(cx, cz),
                1 => draw_crate(cx, cz, h),
                _ => draw_rubble(cx, cz, h),
            }
        }
    }
}

/// A rusty barrel with two hazard-stripe rings (slightly proud so they don't
/// z-fight the body).
fn draw_barrel(cx: f32, cz: f32) {
    let base = vec3(cx, 0.0, cz);
    let body = Color::new(0.46, 0.26, 0.18, 1.0);
    let ring = Color::new(0.85, 0.68, 0.20, 1.0);
    draw_cylinder(base, 0.26, 0.28, 0.74, None, body);
    draw_cylinder(vec3(cx, 0.20, cz), 0.29, 0.29, 0.07, None, ring);
    draw_cylinder(vec3(cx, 0.50, cz), 0.29, 0.29, 0.07, None, ring);
    draw_cylinder_wires(base, 0.26, 0.28, 0.74, None, Color::new(0.0, 0.0, 0.0, 1.0));
}

/// A wooden crate, size jittered a touch by the tile hash.
fn draw_crate(cx: f32, cz: f32, h: u64) {
    let s = 0.30 + hfrac(h) * 0.08;
    let c = vec3(cx, s, cz);
    let size = vec3(s * 2.0, s * 2.0, s * 2.0);
    draw_cube(c, size, None, Color::new(0.42, 0.30, 0.18, 1.0));
    draw_cube_wires(c, size, Color::new(0.18, 0.12, 0.06, 1.0));
}

/// A little scatter of grey rubble cubes, placed from re-mixed hash bits.
fn draw_rubble(cx: f32, cz: f32, h: u64) {
    let col = Color::new(0.30, 0.30, 0.34, 1.0);
    let mut hh = h;
    for _ in 0..3 {
        hh = hh.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1);
        let ox = (hfrac(hh) - 0.5) * 0.6;
        hh = hh.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1);
        let oz = (hfrac(hh) - 0.5) * 0.6;
        let sz = 0.07 + hfrac(hh) * 0.05;
        draw_cube(
            vec3(cx + ox, sz, cz + oz),
            vec3(sz * 2.0, sz * 2.0, sz * 2.0),
            None,
            col,
        );
    }
}

/// Deterministic tile hash (SplitMix64-style mix of tile coords + map seed).
fn tile_hash(tx: u32, ty: u32, seed: u64) -> u64 {
    let mut h = seed
        ^ (tx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (ty as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    h
}

/// Map a hash to a fraction in `[0, 1)` — its low 16 bits.
fn hfrac(h: u64) -> f32 {
    (h & 0xFFFF) as f32 / 65536.0
}

/// Build a small tileable grayscale grain texture in code (no asset file). The
/// value multiplies the per-tile tint at draw time, so it darkens/speckles the
/// flat floor colour into something with grit. Nearest filtering keeps the
/// pixels crisp, matching the boxy look.
fn make_floor_texture() -> Texture2D {
    const N: usize = 16;
    let mut bytes = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let h = tile_hash(x as u32, y as u32, 0xF100_0F1E);
            // Mostly bright with occasional darker specks → subtle grain.
            let mut v = 0.82 + hfrac(h) * 0.18;
            if (h >> 20) % 11 == 0 {
                v *= 0.7;
            }
            let g = (v * 255.0) as u8;
            let i = (y * N + x) * 4;
            bytes[i] = g;
            bytes[i + 1] = g;
            bytes[i + 2] = g;
            bytes[i + 3] = 255;
        }
    }
    let tex = Texture2D::from_rgba8(N as u16, N as u16, &bytes);
    tex.set_filter(FilterMode::Nearest);
    tex
}

/// Floating health bar above a damaged enemy: a dark backing cube with a
/// green→red fill scaled to current/max life. Only drawn once the enemy has
/// taken a hit, so a fresh swarm stays clean. World-X aligned — the fixed
/// follow-cam renders that as a horizontal bar; the fill is nudged toward the
/// camera (+Z) so it doesn't z-fight the backing.
fn draw_health_bar(e: &EnemyInstance, scale: f32) {
    let max = e.combatant.max_life;
    let cur = e.combatant.current_life;
    if max <= 0.0 || cur >= max {
        return;
    }
    let frac = (cur / max).clamp(0.0, 1.0);
    let bar_w = (scale * 2.4).max(0.5);
    let bar_y = figure_top(scale) + 0.16;
    let bar_h = 0.1;
    let bar_d = 0.06;

    // Dark backing, slightly oversized for a border.
    let back_center = vec3(e.x, bar_y, e.y);
    let back_size = vec3(bar_w + 0.04, bar_h + 0.03, bar_d);
    draw_cube(back_center, back_size, None, Color::new(0.05, 0.05, 0.06, 1.0));
    draw_cube_wires(back_center, back_size, Color::new(0.0, 0.0, 0.0, 1.0));

    // Life fill, left-aligned, colored by remaining fraction.
    let fill_w = (bar_w * frac).max(0.001);
    let fill_x = e.x - bar_w * 0.5 + fill_w * 0.5;
    let fill = Color::new((1.0 - frac).min(1.0), frac.min(1.0), 0.15, 1.0);
    draw_cube(
        vec3(fill_x, bar_y, e.y + 0.04),
        vec3(fill_w, bar_h, bar_d),
        None,
        fill,
    );
}

/// (color, scale, torso-width) per enemy archetype. Bigger + redder reads as
/// tankier; `width` fattens the silhouette (the fat zombie is squat and wide).
fn enemy_shape(id: &str) -> (Color, f32, f32) {
    match id {
        "swarm_rusher" => (Color::new(0.80, 0.50, 0.30, 1.0), 0.28, 0.85),
        "fast_zombie" => (Color::new(0.90, 0.70, 0.20, 1.0), 0.30, 0.8),
        "spitter" => (Color::new(0.50, 0.80, 0.40, 1.0), 0.34, 1.0),
        "basic_zombie" => (Color::new(0.75, 0.25, 0.25, 1.0), 0.38, 1.0),
        "fat_zombie" => (Color::new(0.55, 0.20, 0.50, 1.0), 0.52, 1.7),
        "patient_zero" => (Color::new(0.95, 0.10, 0.10, 1.0), 0.80, 1.4),
        _ => (Color::new(0.75, 0.25, 0.25, 1.0), 0.38, 1.0),
    }
}

/// Approximate top of a [`FigureSpec`]'s head for `scale` — where world-aligned
/// overlays (health bar, floating labels) anchor above the figure.
fn figure_top(scale: f32) -> f32 {
    scale * 2.3
}

fn draw_drop(d: &LootDrop) {
    let color = rarity_color(d.item.rarity);
    draw_cube(vec3(d.x, 0.18, d.y), vec3(0.3, 0.3, 0.3), None, color);
    // Loot beam — a vertical line, brighter rarities literally stand taller.
    let beam_top = 1.0 + d.item.rarity.index() as f32 * 0.6;
    draw_line_3d(vec3(d.x, 0.0, d.y), vec3(d.x, beam_top, d.y), color);
}

pub(crate) fn rarity_color(r: Rarity) -> Color {
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
    if p.aoe_radius > 0.0 {
        // Rocket: a chunkier, hotter round so the heavy shot reads at a glance.
        let s = vec3(0.38, 0.38, 0.38);
        draw_cube(c, s, None, Color::new(1.0, 0.5, 0.2, 1.0));
        draw_cube_wires(c, s, Color::new(1.0, 0.85, 0.45, 1.0));
    } else {
        let s = vec3(0.22, 0.22, 0.22);
        draw_cube(c, s, None, Color::new(1.0, 0.9, 0.4, 1.0));
    }
}

/// A rocket detonation: a sphere that swells from the impact and fades over the
/// marker's lifetime, sized to the actual blast radius so the player learns its
/// reach.
fn draw_explosion(e: &Explosion) {
    let frac = (e.ttl / e.max_ttl).clamp(0.0, 1.0); // 1 at impact → 0 at cull
    let progress = 1.0 - frac; // 0 → 1 over the effect's life
    let r = e.radius * (0.35 + 0.65 * progress);
    let c = vec3(e.x, 0.4, e.y);
    draw_sphere(c, r, None, Color::new(1.0, 0.55, 0.15, 0.30 * frac));
    draw_sphere_wires(c, r, None, Color::new(1.0, 0.8, 0.3, 0.85 * frac));
}

/// 2D overlay: health bar, run stats, and the dead screen.
pub fn draw_hud(font: &Font, world: &World) {
    // Health bar, top-left.
    let (bx, by, bw, bh) = (12.0, 12.0, 260.0, 22.0);
    draw_rectangle(bx, by, bw, bh, Color::new(0.15, 0.05, 0.05, 0.9));
    let frac = (world.player.current_life / world.player.max_life).clamp(0.0, 1.0);
    draw_rectangle(bx, by, bw * frac, bh, Color::new(0.85, 0.20, 0.20, 1.0));
    draw_rectangle_lines(bx, by, bw, bh, 2.0, Color::new(0.0, 0.0, 0.0, 1.0));
    crate::ui::text(
        font,
        &format!(
            "{:.0} / {:.0}",
            world.player.current_life, world.player.max_life
        ),
        bx + 8.0,
        by + 16.0,
        18.0,
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
    crate::ui::text(font, &stats, 12.0, 56.0, 20.0, Color::new(0.85, 0.85, 0.85, 1.0));

    if let Some(eq) = world.equipped() {
        let tag = match eq.profile {
            FireProfile::Spread { .. } => {
                format!("   {}-pellet spread", world.tunables.spread_pellets)
            }
            FireProfile::Explosive { .. } => {
                format!("   blast r{:.1}", world.tunables.blast_radius)
            }
            FireProfile::Single => String::new(),
        };
        crate::ui::text(
            font,
            &format!(
                "weapon: {}   dmg {:.0}   rate {:.1}/s{tag}",
                eq.name, eq.weapon.damage_per_shot, eq.weapon.fire_rate
            ),
            12.0,
            78.0,
            17.0,
            Color::new(0.80, 0.80, 0.62, 1.0),
        );

        // Weapon wheel: list the rack, active slot marked, when more than one.
        let rack = world.loadout();
        if rack.len() > 1 {
            let line: Vec<String> = rack
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    let mark = if i == world.active_slot() { "*" } else { "" };
                    format!("{}:{}{mark}", i + 1, w.name)
                })
                .collect();
            crate::ui::text(
                font,
                &line.join("  "),
                12.0,
                98.0,
                16.0,
                Color::new(0.62, 0.66, 0.55, 1.0),
            );
        }
    }

    if let Some(last) = &world.last_pickup {
        crate::ui::text(
            font,
            &format!("picked up: {last}"),
            12.0,
            120.0,
            17.0,
            Color::new(0.7, 0.85, 0.7, 1.0),
        );
    }

    // Controls hint, bottom.
    crate::ui::text(
        font,
        "WASD move  |  aim  |  LMB / Space shoot  |  1-4 / wheel weapon  |  I inventory  |  ESC quit",
        12.0,
        screen_height() - 14.0,
        16.0,
        Color::new(0.55, 0.55, 0.55, 1.0),
    );

    if world.game_over {
        let cx = screen_width() * 0.5;
        let cy = screen_height() * 0.5;
        let msg = "YOU DIED";
        let d = crate::ui::measure(font, msg, 64.0);
        crate::ui::text(font, msg, cx - d.width * 0.5, cy, 64.0, Color::new(0.9, 0.1, 0.1, 1.0));
        let sub = "press R to restart";
        let d2 = crate::ui::measure(font, sub, 28.0);
        crate::ui::text(font, sub, cx - d2.width * 0.5, cy + 40.0, 28.0, WHITE);
    }
}
