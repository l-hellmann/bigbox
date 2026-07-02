//! Bevy shell: window, resources, world tick, follow-camera. The state
//! mutations all live in `bb_game::World`; this file is the runtime adapter.
//!
//! Migration status (macroquad → Bevy): this is **Phase 1** — window + camera +
//! the world-tick loop. Input (Phase 2), 3D rendering (Phase 3), HUD/inventory
//! (Phase 4) and the debug overlay (Phase 5) still live in the inherited
//! macroquad modules below and are ported in later phases. Until Phase 3 lands
//! nothing is drawn to screen — the world ticks headless behind the camera.
//! See `.attic/migration/plan.md`.
//!
//! Rendering is **boxy 3D** (BoxHead-style): the world is a 2D plane in
//! gameplay terms, drawn in 3D space with extruded-cube walls and an angled
//! follow-camera. Coordinate convention: a gameplay tile position `(x, y)`
//! maps to world `(x, _, y)` — world **X = tile x**, world **Z = tile y**,
//! world **Y = up**. So gameplay coords need no translation; only height (Y)
//! is invented here at the render layer.
//!
//! Content (enemy roster, base items, affixes) is embedded at build time via
//! `include_str!`, so a shipped native binary is self-contained.

use bb_game::{Content, World};
use bb_procgen::{ArenaParams, Map, MapParams, generate_arena, generate_bsp};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

// Inherited macroquad modules — not yet wired into the Bevy app. They stay
// compiled (so they don't bitrot) until their porting phase; `allow(dead_code)`
// silences the transitional "unused" noise. Removed file-by-file as each phase
// rebuilds it as Bevy systems.
#[cfg(feature = "debug")]
#[allow(dead_code)]
mod debug;
mod input;
mod pad;
// Inherited macroquad render/HUD — its 2D HUD + debug viz are ported in Phases
// 4/5; the 3D scene has moved to `scene` (Bevy). Kept as reference until then.
#[allow(dead_code)]
mod render;
mod scene;
#[allow(dead_code)]
mod ui;

// Re-export the gamepad diagnostic type so `crate::PadDiag` (used by the debug
// overlay) keeps resolving from the crate root.
#[cfg(feature = "debug")]
#[allow(unused_imports)]
pub use pad::PadDiag;

/// Camera offset above the player, in world units. With `CAMERA_BACK` this
/// sets the tilt: atan(HEIGHT / BACK) ≈ 56° — the classic BoxHead overhead
/// angle that still shows wall height.
const CAMERA_HEIGHT: f32 = 15.0;
/// Camera offset toward +Z (screen-down / "south") from the player. The cam
/// looks down-and-forward at the player from here.
const CAMERA_BACK: f32 = 10.0;

/// Map seed from `BB_SEED`, if set and parseable.
fn map_seed_env() -> Option<u64> {
    std::env::var("BB_SEED").ok().and_then(|s| s.parse().ok())
}

/// Content is **embedded at build time** via `include_str!`, not read from
/// disk — keeps a shipped native binary self-contained, no content directory to
/// ship alongside it. A parse failure here means malformed RON in the tree,
/// i.e. a build-time content bug, so panicking is correct.
fn load_content() -> Content {
    Content {
        enemies: bb_content::parse_enemies("enemies.ron", include_str!("../../content/data/enemies.ron"))
            .expect("embedded enemies.ron"),
        bases: bb_content::parse_bases("bases.ron", include_str!("../../content/data/bases.ron"))
            .expect("embedded bases.ron"),
        affixes: bb_content::parse_affixes("affixes.ron", include_str!("../../content/data/affixes.ron"))
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

/// Parse a level name (the `arena` / `arena-empty` tokens from `BB_LEVEL`).
/// Anything else → the BSP dungeon. Non-debug builds only — the debug build
/// takes the level through clap's `LevelArg`.
#[cfg(not(feature = "debug"))]
fn level_from_str(s: &str) -> Level {
    match s {
        "arena" => Level::Arena,
        "arena-empty" => Level::ArenaEmpty,
        _ => Level::Bsp,
    }
}

/// CLI surface for local dev — only compiled under the `debug` feature, so the
/// prod build pulls in no clap. The `cargo arena` / `arena-empty` / `dbg`
/// aliases drive this; everything has an env-var or default fallback so a bare
/// `cargo run` still works.
#[cfg(feature = "debug")]
#[derive(clap::Parser)]
#[command(name = "bb-game", about = "bigbox runtime (debug build)")]
struct Cli {
    /// Starting level.
    #[arg(value_enum, default_value_t = LevelArg::Bsp)]
    level: LevelArg,
    /// Map seed (overrides BB_SEED; default 42).
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
/// non-debug reads `BB_LEVEL` / `BB_SEED`. Each path falls back to the BSP
/// dungeon at seed 42.
#[cfg(feature = "debug")]
fn resolve_run() -> (Level, u64) {
    use clap::Parser;
    let cli = Cli::parse();
    (cli.level.into(), cli.seed.or_else(map_seed_env).unwrap_or(42))
}

#[cfg(not(feature = "debug"))]
fn resolve_run() -> (Level, u64) {
    let level = std::env::var("BB_LEVEL")
        .map(|s| level_from_str(&s))
        .unwrap_or(Level::Bsp);
    (level, map_seed_env().unwrap_or(42))
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

// ---- Bevy resources ----

/// The engine-agnostic simulation, wrapped so `lib.rs` stays free of Bevy
/// derives. One `World::tick` per frame drives everything.
#[derive(Resource)]
struct Sim(World);

/// Embedded content (enemies / bases / affixes), read-only after load.
#[derive(Resource)]
struct GameContent(Content);

/// The level + seed for this run, kept so a restart can rebuild the world.
#[derive(Resource, Clone, Copy)]
struct RunConfig {
    level: Level,
    seed: u64,
}

/// Inventory-open pause gate. The inventory modal (Phase 4) suspends the sim
/// while open; for now it defaults unpaused and nothing toggles it.
#[derive(Resource)]
struct Paused(bool);

/// This frame's resolved aim, shared from `player_input` to the renderer. `yaw`
/// faces the player figure at the cursor/stick; `hit` is the ground point the
/// aim line/crosshair marks. `yaw` holds its last value when aim is momentarily
/// absent so the figure doesn't snap to north.
#[derive(Resource, Default)]
struct Aim {
    yaw: f32,
    hit: Option<Vec3>,
}

/// Marks the follow-camera so `camera_follow` can re-target it each frame.
#[derive(Component)]
struct FollowCam;

/// System set wrapping the world tick, so Phase 2 input (before) and the
/// camera-follow (after) can order around it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct SimSet;

fn main() {
    let (level, seed) = resolve_run();

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bigbox".into(),
                resolution: (1280, 720).into(),
                ..default()
            }),
            ..default()
        }))
        // The old macroquad clear color.
        .insert_resource(ClearColor(Color::srgb(0.02, 0.02, 0.03)))
        .insert_resource(RunConfig { level, seed })
        .insert_resource(Paused(false))
        .init_resource::<Aim>()
        .init_resource::<scene::ProjectilePool>()
        // World + camera exist first, then render assets, then the static map
        // geometry (which reads both).
        .add_systems(
            Startup,
            (setup, scene::setup_render_assets, scene::spawn_map).chain(),
        )
        .configure_sets(Update, SimSet.run_if(not_paused))
        // Input feeds the command stream, then the world ticks — ordered.
        .add_systems(Update, (player_input, tick_world).chain().in_set(SimSet))
        // Renderer reads post-tick state: reconcile entities + follow the cam.
        // Enemies + projectiles are pooled; the rest reconcile naively.
        .add_systems(
            Update,
            (
                camera_follow,
                scene::sync_enemies,
                scene::sync_projectiles,
                scene::sync_misc,
                scene::draw_aim,
            )
                .after(SimSet),
        );

    // Dev-only headless render validation: capture a screenshot then exit.
    #[cfg(feature = "screenshot")]
    app.add_systems(Update, screenshot_once);

    app.run();
}

/// Startup: load content, build the world, spawn the follow-camera over the
/// player spawn. World + content become resources.
fn setup(mut commands: Commands, run: Res<RunConfig>) {
    let content = load_content();
    let world = new_world(run.level, run.seed, &content);

    let (eye, target) = camera_pose(world.player.x, world.player.y);
    commands.spawn((
        Camera3d::default(),
        Transform::from_translation(eye).looking_at(target, Vec3::Y),
        // Flat, unfiltered color — the boxy materials are unlit, so reproduce
        // macroquad's look with no tonemapping curve applied.
        Tonemapping::None,
        FollowCam,
    ));

    commands.insert_resource(GameContent(content));
    commands.insert_resource(Sim(world));
    // The mouse-vs-pad aim latch, carried across frames. Seeded at origin; the
    // first real cursor position reclaims the mouse source harmlessly.
    commands.insert_resource(input::AimState::new((0.0, 0.0)));
}

/// Run condition: sim + camera advance only while unpaused.
fn not_paused(paused: Res<Paused>) -> bool {
    !paused.0
}

/// Device input → `Command` stream → `World::apply`. Resolves the aim latch
/// (mouse vs. pad), then builds and applies this frame's commands. Ordered
/// before `tick_world` so the tick sees this frame's intent. Runs in `SimSet`,
/// so the inventory-pause gate suspends input alongside the tick.
#[allow(clippy::too_many_arguments)]
fn player_input(
    mut sim: ResMut<Sim>,
    mut aim_state: ResMut<input::AimState>,
    mut aim: ResMut<Aim>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut wheel: MessageReader<MouseWheel>,
    gamepads: Query<&Gamepad>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<FollowCam>>,
) {
    let dt = time.delta_secs();
    let pad = pad::read_pad(&gamepads, sim.0.tunables.stick_deadzone);

    // Cursor → ground-plane hit for mouse aim. `None` when the cursor is outside
    // the window; hold the last position so that absence doesn't read as motion.
    let cursor = windows.iter().next().and_then(|w| w.cursor_position());
    let mouse_hit = match (cursor, cameras.iter().next()) {
        (Some(c), Some((cam, xf))) => ground_hit(cam, xf, c),
        _ => None,
    };
    let mouse_pos = cursor
        .map(|c| (c.x, c.y))
        .unwrap_or_else(|| aim_state.last_mouse());

    let frame = aim_state.update(&sim.0.player, &pad, mouse_pos, mouse_hit);

    // Publish the resolved aim to the renderer: face the figure, mark the hit.
    // Keep the last yaw when aim is momentarily absent (no snap to north).
    if let Some((dx, dy)) = frame.dir {
        aim.yaw = dx.atan2(dy);
    }
    aim.hit = frame.hit;

    // Accumulate this frame's wheel delta (event-based in Bevy).
    let wheel_y: f32 = wheel.read().map(|e| e.y).sum();

    for cmd in input::collect_commands(frame.dir, &pad, &keys, &mouse_buttons, wheel_y) {
        sim.0.apply(cmd, dt);
    }
}

/// Cast a ray from the camera through the cursor and intersect the ground plane
/// (`Y = 0`). Uses Bevy's `viewport_to_world`; guards a ray parallel to / below
/// the plane. Returns the world-space hit point.
fn ground_hit(camera: &Camera, cam_xf: &GlobalTransform, cursor: Vec2) -> Option<Vec3> {
    let ray = camera.viewport_to_world(cam_xf, cursor).ok()?;
    if ray.direction.y.abs() < 1e-6 {
        return None;
    }
    let t = -ray.origin.y / ray.direction.y;
    if t <= 0.0 {
        return None;
    }
    Some(ray.origin + ray.direction * t)
}

/// One `World::tick` per frame, after `player_input` has drained the command
/// stream. Advances waves, enemy steering, projectiles, loot on `dt`.
fn tick_world(mut sim: ResMut<Sim>, content: Res<GameContent>, time: Res<Time>) {
    sim.0.tick(time.delta_secs(), &content.0);
}

/// Re-target the follow-cam from the *post-tick* player position (ordered after
/// `SimSet`) so the world doesn't lag the player by one frame. Orientation is
/// fixed; only translation tracks — matching the old macroquad behaviour.
fn camera_follow(sim: Res<Sim>, mut cam: Query<&mut Transform, With<FollowCam>>) {
    let p = &sim.0.player;
    let (eye, target) = camera_pose(p.x, p.y);
    if let Ok(mut t) = cam.single_mut() {
        *t = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    }
}

/// Dev-only (`screenshot` feature): once `BB_SHOT` is set, wait for the scene to
/// populate then save one screenshot of the primary window and exit. Headless
/// render validation for CI / agents. The capture frame defaults to 120 (gives
/// the sim time to spawn a wave and the renderer time to reconcile) and can be
/// overridden with `BB_SHOT_FRAME` — e.g. a later frame to let a swarm converge.
#[cfg(feature = "screenshot")]
fn screenshot_once(mut commands: Commands, mut frame: Local<u32>, mut exit: MessageWriter<AppExit>) {
    let Ok(path) = std::env::var("BB_SHOT") else {
        return;
    };
    let at = std::env::var("BB_SHOT_FRAME")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);
    *frame += 1;
    if *frame == at {
        use bevy::render::view::screenshot::{Screenshot, save_to_disk};
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if *frame >= at + 20 {
        exit.write(AppExit::Success);
    }
}

/// Eye + look-at target for the angled follow-cam given a player tile position.
/// Eye = `(px, HEIGHT, py + BACK)`, target = `(px, 0, py)`, up = Y.
fn camera_pose(px: f32, py: f32) -> (Vec3, Vec3) {
    let target = Vec3::new(px, 0.0, py);
    let eye = target + Vec3::new(0.0, CAMERA_HEIGHT, CAMERA_BACK);
    (eye, target)
}
