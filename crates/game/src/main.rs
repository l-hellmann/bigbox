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

use h2b_game::{Command, Content, Player, World};
use h2b_procgen::{ArenaParams, Map, MapParams, generate_arena, generate_bsp};
use macroquad::prelude::*;

#[cfg(feature = "debug")]
mod debug;
mod input;
mod pad;
mod render;

// Re-export the gamepad diagnostic type so `crate::PadDiag` (used by the debug
// overlay) keeps resolving from the crate root after the move into `pad`.
#[cfg(feature = "debug")]
pub use pad::PadDiag;

// On wasm, `rand`'s transitive `getrandom` needs a backend or it won't link.
// Gameplay RNG is always seeded, so getrandom is never actually called —
// register a trivial stub (deterministic zeros) rather than the `js` feature,
// which would pull wasm-bindgen glue that macroquad's loader can't satisfy.
#[cfg(target_arch = "wasm32")]
getrandom::register_custom_getrandom!(getrandom_stub);
#[cfg(target_arch = "wasm32")]
fn getrandom_stub(buf: &mut [u8]) -> Result<(), getrandom::Error> {
    buf.fill(0);
    Ok(())
}

/// Camera offset above the player, in world units. With `CAMERA_BACK` this
/// sets the tilt: atan(HEIGHT / BACK) ≈ 56° — the classic BoxHead overhead
/// angle that still shows wall height.
const CAMERA_HEIGHT: f32 = 15.0;
/// Camera offset toward +Z (screen-down / "south") from the player. The cam
/// looks down-and-forward at the player from here.
const CAMERA_BACK: f32 = 10.0;

/// Map seed from `H2B_SEED`, if set and parseable. Native only — the web build
/// reads the seed from the URL instead (see `resolve_run`).
#[cfg(not(target_arch = "wasm32"))]
fn map_seed_env() -> Option<u64> {
    std::env::var("H2B_SEED").ok().and_then(|s| s.parse().ok())
}

fn window_conf() -> Conf {
    Conf {
        window_title: "head2box".into(),
        window_width: 1280,
        window_height: 720,
        // Render at the native (retina) framebuffer resolution. macroquad keeps
        // its 2D coordinate + mouse API logical regardless, so the HUD layout is
        // unchanged — but it gives miniquad/egui the correct DPI scale, which
        // fixes egui pointer mapping (dropped clicks) on high-DPI displays.
        high_dpi: true,
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

/// Starting level: `arena` (open debug room with pillars), `arena-empty` (no
/// pillars), or the regular BSP dungeon. The debug overlay can also swap maps
/// live at runtime.
#[derive(Clone, Copy)]
enum Level {
    Bsp,
    Arena,
    ArenaEmpty,
}

/// Parse a level name (the `arena` / `arena-empty` tokens shared by `H2B_LEVEL`
/// and the web `?level=` query). Anything else → the BSP dungeon. Non-debug
/// builds only — the debug build takes the level through clap's `LevelArg`.
#[cfg(not(feature = "debug"))]
fn level_from_str(s: &str) -> Level {
    match s {
        "arena" => Level::Arena,
        "arena-empty" => Level::ArenaEmpty,
        _ => Level::Bsp,
    }
}

/// CLI surface for local dev — only compiled under the `debug` feature, so the
/// prod/wasm build pulls in no clap. The `cargo arena` / `arena-empty` / `dbg`
/// aliases drive this; everything has an env-var or default fallback so a bare
/// `cargo run` still works.
#[cfg(feature = "debug")]
#[derive(clap::Parser)]
#[command(name = "h2b-game", about = "head2box runtime (debug build)")]
struct Cli {
    /// Starting level.
    #[arg(value_enum, default_value_t = LevelArg::Bsp)]
    level: LevelArg,
    /// Map seed (overrides H2B_SEED; default 42).
    #[arg(long)]
    seed: Option<u64>,
}

#[cfg(feature = "debug")]
#[derive(Clone, Copy, clap::ValueEnum)]
enum LevelArg {
    Bsp,
    Arena,
    ArenaEmpty,
}

#[cfg(feature = "debug")]
impl From<LevelArg> for Level {
    fn from(l: LevelArg) -> Self {
        match l {
            LevelArg::Bsp => Level::Bsp,
            LevelArg::Arena => Level::Arena,
            LevelArg::ArenaEmpty => Level::ArenaEmpty,
        }
    }
}

/// Resolve `(level, seed)` for this run. Debug builds parse the CLI via clap;
/// non-debug native reads `H2B_LEVEL` / `H2B_SEED`; web reads `?level=&seed=`
/// from the page URL. Each path falls back to the BSP dungeon at seed 42.
#[cfg(feature = "debug")]
fn resolve_run() -> (Level, u64) {
    use clap::Parser;
    let cli = Cli::parse();
    (cli.level.into(), cli.seed.or_else(map_seed_env).unwrap_or(42))
}

#[cfg(all(not(feature = "debug"), not(target_arch = "wasm32")))]
fn resolve_run() -> (Level, u64) {
    let level = std::env::var("H2B_LEVEL")
        .map(|s| level_from_str(&s))
        .unwrap_or(Level::Bsp);
    (level, map_seed_env().unwrap_or(42))
}

/// Web: read run config from the page URL query string via quad-url, so a URL
/// like `index.html?seed=123&level=arena` reproduces an exact run. quad-url maps
/// `?seed=123` to the arg `--seed=123`, which `easy_parse` splits back out.
#[cfg(all(not(feature = "debug"), target_arch = "wasm32"))]
fn resolve_run() -> (Level, u64) {
    let mut level = Level::Bsp;
    let mut seed = None;
    for param in quad_url::get_program_parameters() {
        match quad_url::easy_parse(&param) {
            Some(("seed", Some(v))) => seed = v.parse().ok(),
            Some(("level", Some(v))) => level = level_from_str(v),
            _ => {}
        }
    }
    (level, seed.unwrap_or(42))
}

fn build_map(level: Level, seed: u64) -> Map {
    match level {
        Level::Arena => generate_arena(&ArenaParams {
            seed,
            ..Default::default()
        }),
        Level::ArenaEmpty => generate_arena(&ArenaParams {
            seed,
            pillars: false,
            ..Default::default()
        }),
        Level::Bsp => generate_bsp(&MapParams {
            seed,
            ..Default::default()
        }),
    }
}

fn new_world(level: Level, seed: u64, content: &Content) -> World {
    let mut world = World::new(build_map(level, seed));
    // The arena is the controlled-testing level: start with waves suspended so
    // you spawn enemies deliberately rather than getting swarmed on entry.
    if matches!(level, Level::Arena | Level::ArenaEmpty) {
        world.tunables.auto_spawn = false;
    }
    // Arm the player so the fire path is gear-derived from frame one. A stock
    // pistol's stats match the historical constant defaults, so this changes no
    // numbers — it just makes "what am I holding" explicit and pickup-swappable.
    world.equip_base("pistol", content);
    world
}

#[macroquad::main(window_conf)]
async fn main() {
    let (level, seed) = resolve_run();
    let content = load_content();
    let mut world = new_world(level, seed, &content);

    #[cfg(feature = "debug")]
    let mut dbg = debug::DebugUi::new();

    let mut pads = pad::Pads::new();

    // Aim-source latch (mouse vs. pad), carried across frames.
    let mut aim_state = input::AimState::new(mouse_position());

    loop {
        let dt = get_frame_time();

        if is_key_pressed(KeyCode::Escape) {
            break;
        }
        // Restart from the dead screen.
        if world.game_over && is_key_pressed(KeyCode::R) {
            world = new_world(level, seed, &content);
        }

        // Build the follow-cam first: aim raycasting needs it to unproject
        // the cursor onto the ground plane.
        let camera = build_camera(&world.player);
        let pad = pads.read(world.tunables.stick_deadzone);
        let mouse_hit = ground_hit(&camera);

        // Resolve this frame's aim (mouse-vs-pad latch + 3D crosshair point).
        let aim = aim_state.update(&world.player, &pad, mouse_position(), mouse_hit);

        // Debug overlay runs before input so its spawns/tunable edits apply to
        // this frame's tick, and so it can swallow clicks that land on the
        // panel (`block_fire`).
        #[cfg(feature = "debug")]
        let block_fire = {
            dbg.handle_toggle();
            let pad_diag = pads.debug_diag();
            dbg.run(&mut world, &content, aim.hit.map(|h| (h.x, h.z)), &pad_diag)
        };
        #[cfg(not(feature = "debug"))]
        let block_fire = false;

        for cmd in input::collect_input(aim.dir, &pad) {
            if block_fire && matches!(cmd, Command::Fire { .. }) {
                continue;
            }
            world.apply(cmd, dt);
        }
        world.tick(dt, &content);

        // ---- 3D pass ----
        clear_background(Color::new(0.02, 0.02, 0.03, 1.0));
        set_camera(&camera);
        render::draw_scene(&world, &content, aim.hit);
        #[cfg(feature = "debug")]
        if dbg.show_flow() {
            render::draw_flow_field(&world);
        }

        // ---- 2D overlay pass ----
        set_default_camera();
        render::draw_hud(&world);
        #[cfg(feature = "debug")]
        if dbg.show_entity_stats() {
            render::draw_entity_stats(&world, &content, &camera);
        }

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
