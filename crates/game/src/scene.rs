//! Boxy-3D rendering, Bevy port (BoxHead-style). The world is a 2D plane in
//! gameplay terms, drawn in 3D with extruded-cube walls and stacked-cube
//! "box-people", under an angled follow-cam. Coordinate convention matches the
//! sim: tile `(x, y)` → world `(x, _, y)`, world Y up.
//!
//! Strategy (see `.attic/migration/phase-3-rendering`): **static geometry**
//! (floor / walls / props) is spawned once on map load and left for Bevy to
//! cull; **dynamic entities** are reconciled against `World` each frame. Almost
//! everything is now **pooled / persistent** — enemies and the player figure
//! (keyed by id / marker: spawn once, update `Transform` + limb swing), health
//! bars (keyed by enemy id, only the fill updates), projectiles (index pool),
//! and explosions (spawned from a sim event, then self-culling render-side). The
//! only remaining despawn-all/respawn-all reconcile is loot drops, which are few
//! and change rarely. Persisting entities (instead of rebuilding hierarchies per
//! frame) keeps Bevy's batching stable and avoids per-frame archetype churn.
//!
//! Materials are **unlit** (flat, no lights) to reproduce macroquad's look; the
//! lighting/PBR epic is deferred. Shared mesh + material handles (via a
//! color-keyed cache) let Bevy auto-batch, per the CLAUDE.md hot-path rule.

use crate::{GameContent, Sim};
use bb_core::Rarity;
use bb_game::{EnemyInstance, LootDrop};
use bb_procgen::{Map, Tile};
use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageSampler};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// How tall wall cubes stand, in world units (= tiles).
const WALL_HEIGHT: f32 = 1.6;
/// Player figure scale (≈ the old cube half-extent).
const PLAYER_HALF: f32 = 0.35;
/// Radians of walk-cycle phase per tile of ground distance covered. The phase is
/// advanced per-figure by `distance_moved × WALK_FREQ` (see [`WalkCycle`]), so
/// stride tracks actual motion and each figure animates independently.
const WALK_FREQ: f32 = 6.0;
/// How long a rocket's blast sphere stays on screen (seconds) — the render-side
/// clock for the fading effect. The sim only emits the `ExplosionEvent`; the
/// lifetime lives here so ephemeral visuals never touch replicated world state.
const EXPLOSION_TTL: f32 = 0.35;

const FLOOR_DARK: Color = Color::srgb(0.07, 0.07, 0.09);
const FLOOR_LIGHT: Color = Color::srgb(0.11, 0.11, 0.14);
const WALL_COLOR: Color = Color::srgb(0.20, 0.20, 0.24);

// ---- Shared render assets ------------------------------------------------

/// Meshes + textures + a color-keyed material cache, built once at startup and
/// reused every frame. Keeping handles shared is what lets Bevy instance/batch.
#[derive(Resource)]
pub struct RenderAssets {
    /// Unit cube (1×1×1, centered) — scaled per-instance via `Transform`.
    cube: Handle<Mesh>,
    /// Unit XZ quad (1×1, +Y normal, UV 0..1) for floor tiles.
    plane: Handle<Mesh>,
    /// Unit cylinder (radius 0.5, height 1.0) for barrels.
    cylinder: Handle<Mesh>,
    /// Unit sphere (radius 0.5) for explosion blasts.
    sphere: Handle<Mesh>,
    /// Floor grain, sampled per tile and tinted per checker square.
    floor_light: Handle<StandardMaterial>,
    floor_dark: Handle<StandardMaterial>,
}

/// Color-keyed unlit-material cache, shared across the spawn + sync systems so a
/// given color mints exactly one `StandardMaterial` for the whole run.
#[derive(Resource, Default)]
pub struct MatCache(HashMap<u32, Handle<StandardMaterial>>);

/// Bundles the cache + the material asset store so spawn helpers can resolve a
/// color to a shared handle in one call.
struct Painter<'a> {
    cache: &'a mut MatCache,
    materials: &'a mut Assets<StandardMaterial>,
}

impl Painter<'_> {
    /// Shared unlit material for `color` (alpha < 1 → alpha-blended). Reused
    /// across frames and entities so batching kicks in.
    fn mat(&mut self, color: Color) -> Handle<StandardMaterial> {
        let s = color.to_srgba();
        let key = u32::from_le_bytes([
            (s.red * 255.0) as u8,
            (s.green * 255.0) as u8,
            (s.blue * 255.0) as u8,
            (s.alpha * 255.0) as u8,
        ]);
        self.cache
            .0
            .entry(key)
            .or_insert_with(|| {
                self.materials.add(StandardMaterial {
                    base_color: color,
                    unlit: true,
                    alpha_mode: if s.alpha < 1.0 {
                        AlphaMode::Blend
                    } else {
                        AlphaMode::Opaque
                    },
                    ..default()
                })
            })
            .clone()
    }
}

// ---- Markers -------------------------------------------------------------

/// Static map geometry (floor / walls / props). Despawned + respawned on a
/// map-swap (debug overlay, Phase 5).
#[derive(Component)]
pub struct MapGeometry;

/// Loot drops — the last remaining naive-reconcile dynamics (a handful on the
/// ground, changing only on pickup/spawn). Cleared and respawned each frame; the
/// churn is trivial. Everything swarm-heavy or hierarchical (enemies, the player
/// figure, projectiles, health bars, explosions) is **pooled / persistent**
/// instead (see [`EnemyView`] / [`PlayerView`] / [`ProjectilePool`] /
/// [`HealthBarView`] / [`ExplosionEffect`]) per the CLAUDE.md hot-path rule, so
/// they are *not* tagged with this.
#[derive(Component)]
pub struct DynamicEntity;

/// Marks a pooled enemy figure's parent, keyed by the stable `EnemyInstance.id`
/// so the figure (parent + child cubes) persists across frames — only its
/// `Transform` and limb swing update, no per-frame spawn/despawn.
#[derive(Component)]
pub struct EnemyView(u64);

/// Marks the single persistent player figure. Like [`EnemyView`] it spawns once
/// and thereafter only its root `Transform` (position + aim yaw) and limb swing
/// update each frame — no despawn/respawn of the ~11-cube hierarchy.
#[derive(Component)]
pub struct PlayerView;

/// Marks a pooled health-bar's parent, keyed by the owning enemy id. World-
/// aligned (never rotated), it persists while its enemy is damaged and despawns
/// when the enemy heals to full or dies. Only the fill child updates per frame.
#[derive(Component)]
pub struct HealthBarView(u64);

/// The green→red fill child of a [`HealthBarView`]. Carries its **own** unique
/// material (not the shared color cache) so its continuously-varying color can
/// be mutated in place each frame without minting a fresh cached material per
/// distinct HP fraction.
#[derive(Component)]
pub struct HealthBarFill;

/// A render-side rocket-blast sphere, spawned from an `ExplosionEvent` and
/// self-culling on its own `ttl` clock. Owns a unique fading material (freed
/// with the entity on despawn). The sim never sees or ticks these — the whole
/// effect (spawn → expand → fade → despawn) lives render-side.
#[derive(Component)]
pub struct ExplosionEffect {
    ttl: f32,
    max_ttl: f32,
    radius: f32,
}

/// Per-figure walk-cycle phase, advanced by ground distance moved so the stride
/// tracks real motion (planted feet when idle) and each figure animates on its
/// own clock rather than off absolute world coordinates.
#[derive(Component)]
pub struct WalkCycle {
    phase: f32,
    last: Vec2,
}

/// Advance a figure's walk phase by the distance it moved to `pos` and return
/// the current swing (`sin(phase)`). Shared by the enemy and player reconcilers.
fn advance_walk(cycle: &mut WalkCycle, pos: Vec2) -> f32 {
    let dist = (pos - cycle.last).length();
    cycle.phase += dist * WALK_FREQ;
    cycle.last = pos;
    cycle.phase.sin()
}

/// A swinging limb (leg/arm) of a box figure. `amp_z` is the signed fore/aft
/// amplitude; the walk-bob sets local `translation.z = amp_z * sin(phase)`.
#[derive(Component)]
pub struct Limb {
    amp_z: f32,
}

/// A pooled projectile cube. Reused by index across frames.
#[derive(Component)]
pub struct ProjectileMarker;

/// Entity pool for projectile cubes — grown/shrunk only when the projectile
/// count changes, positions updated in place otherwise.
#[derive(Resource, Default)]
pub struct ProjectilePool(Vec<Entity>);

// ---- Startup: assets -----------------------------------------------------

/// Build the shared meshes, floor grain image, and the two floor materials.
pub fn setup_render_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let plane = meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(0.5)));
    let cylinder = meshes.add(Cylinder::new(0.5, 1.0));
    let sphere = meshes.add(Sphere::new(0.5));

    let grain = images.add(make_floor_image());
    let floor_mat = |materials: &mut Assets<StandardMaterial>, tint: Color| {
        materials.add(StandardMaterial {
            base_color: tint,
            base_color_texture: Some(grain.clone()),
            unlit: true,
            ..default()
        })
    };
    let floor_light = floor_mat(&mut materials, FLOOR_LIGHT);
    let floor_dark = floor_mat(&mut materials, FLOOR_DARK);

    commands.insert_resource(RenderAssets {
        cube,
        plane,
        cylinder,
        sphere,
        floor_light,
        floor_dark,
    });
    commands.init_resource::<MatCache>();
}

// ---- Startup: static geometry -------------------------------------------

/// Spawn the floor grid, wall cubes, and decorative props once from the loaded
/// map. All tagged [`MapGeometry`].
pub fn spawn_map(
    mut commands: Commands,
    sim: Res<Sim>,
    assets: Res<RenderAssets>,
    mut cache: ResMut<MatCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut painter = Painter {
        cache: &mut cache,
        materials: &mut materials,
    };
    build_map_geometry(&mut commands, &sim.0.map, &assets, &mut painter);
}

/// Respawn the static map geometry after a debug map-swap: despawn the old
/// [`MapGeometry`], rebuild from the (already-reloaded) world map. Debug-only —
/// the only path that mutates the map at runtime.
#[cfg(feature = "debug")]
pub fn reload_map_geometry(
    mut commands: Commands,
    sim: Res<Sim>,
    assets: Res<RenderAssets>,
    mut cache: ResMut<MatCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut dirty: ResMut<crate::debug::MapDirty>,
    existing: Query<Entity, With<MapGeometry>>,
) {
    if !dirty.0 {
        return;
    }
    dirty.0 = false;
    for e in &existing {
        commands.entity(e).despawn();
    }
    let mut painter = Painter {
        cache: &mut cache,
        materials: &mut materials,
    };
    build_map_geometry(&mut commands, &sim.0.map, &assets, &mut painter);
}

/// Spawn the floor grid, wall cubes, and props for `map`, all tagged
/// [`MapGeometry`]. Shared by initial load and debug map-swap.
fn build_map_geometry(commands: &mut Commands, map: &Map, assets: &RenderAssets, painter: &mut Painter) {
    // Floor: one textured checker quad per tile, tinted by (tx+ty) parity.
    for ty in 0..map.height {
        for tx in 0..map.width {
            let cx = tx as f32 + 0.5;
            let cz = ty as f32 + 0.5;
            let mat = if (tx + ty) % 2 == 0 {
                assets.floor_light.clone()
            } else {
                assets.floor_dark.clone()
            };
            commands.spawn((
                Mesh3d(assets.plane.clone()),
                MeshMaterial3d(mat),
                Transform::from_xyz(cx, 0.005, cz),
                MapGeometry,
            ));
        }
    }

    // Walls: extruded cubes on Wall tiles.
    let wall_mat = painter.mat(WALL_COLOR);
    for ty in 0..map.height {
        for tx in 0..map.width {
            if !matches!(map.tile_at(tx, ty), Tile::Wall) {
                continue;
            }
            let cx = tx as f32 + 0.5;
            let cz = ty as f32 + 0.5;
            commands.spawn((
                Mesh3d(assets.cube.clone()),
                MeshMaterial3d(wall_mat.clone()),
                Transform::from_xyz(cx, WALL_HEIGHT * 0.5, cz)
                    .with_scale(Vec3::new(1.0, WALL_HEIGHT, 1.0)),
                MapGeometry,
            ));
        }
    }

    // Props: deterministic scatter from the map seed (render-only, no collision).
    spawn_props(commands, assets, painter, map);
}

/// Scatter sparse decorative props on floor tiles, chosen deterministically
/// from `(tile, map.seed)` — same map always dresses identically.
fn spawn_props(commands: &mut Commands, assets: &RenderAssets, painter: &mut Painter, map: &Map) {
    for ty in 0..map.height {
        for tx in 0..map.width {
            if !matches!(map.tile_at(tx, ty), Tile::Floor) {
                continue;
            }
            if (tx, ty) == map.player_spawn {
                continue;
            }
            let cx = tx as f32 + 0.5;
            let cz = ty as f32 + 0.5;
            let h = tile_hash(tx, ty, map.seed);
            if h % 17 != 0 {
                continue;
            }
            match (h >> 8) % 3 {
                0 => spawn_barrel(commands, assets, painter, cx, cz),
                1 => spawn_crate(commands, assets, painter, cx, cz, h),
                _ => spawn_rubble(commands, assets, painter, cx, cz, h),
            }
        }
    }
}

fn spawn_barrel(
    commands: &mut Commands,
    assets: &RenderAssets,
    painter: &mut Painter,
    cx: f32,
    cz: f32,
) {
    let body = painter.mat(Color::srgb(0.46, 0.26, 0.18));
    let ring = painter.mat(Color::srgb(0.85, 0.68, 0.20));
    // Body: unit cylinder scaled to r≈0.27, height 0.74, sitting on the floor.
    commands.spawn((
        Mesh3d(assets.cylinder.clone()),
        MeshMaterial3d(body),
        Transform::from_xyz(cx, 0.37, cz).with_scale(Vec3::new(0.54, 0.74, 0.54)),
        MapGeometry,
    ));
    for y in [0.20_f32, 0.50] {
        commands.spawn((
            Mesh3d(assets.cylinder.clone()),
            MeshMaterial3d(ring.clone()),
            Transform::from_xyz(cx, y, cz).with_scale(Vec3::new(0.58, 0.07, 0.58)),
            MapGeometry,
        ));
    }
}

fn spawn_crate(
    commands: &mut Commands,
    assets: &RenderAssets,
    painter: &mut Painter,
    cx: f32,
    cz: f32,
    h: u64,
) {
    let s = 0.30 + hfrac(h) * 0.08;
    let mat = painter.mat(Color::srgb(0.42, 0.30, 0.18));
    commands.spawn((
        Mesh3d(assets.cube.clone()),
        MeshMaterial3d(mat),
        Transform::from_xyz(cx, s, cz).with_scale(Vec3::splat(s * 2.0)),
        MapGeometry,
    ));
}

fn spawn_rubble(
    commands: &mut Commands,
    assets: &RenderAssets,
    painter: &mut Painter,
    cx: f32,
    cz: f32,
    h: u64,
) {
    let mat = painter.mat(Color::srgb(0.30, 0.30, 0.34));
    let mut hh = h;
    for _ in 0..3 {
        hh = hh.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1);
        let ox = (hfrac(hh) - 0.5) * 0.6;
        hh = hh.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1);
        let oz = (hfrac(hh) - 0.5) * 0.6;
        let sz = 0.07 + hfrac(hh) * 0.05;
        commands.spawn((
            Mesh3d(assets.cube.clone()),
            MeshMaterial3d(mat.clone()),
            Transform::from_xyz(cx + ox, sz, cz + oz).with_scale(Vec3::splat(sz * 2.0)),
            MapGeometry,
        ));
    }
}

// ---- Per-frame: dynamic entities ----------------------------------------

/// Pooled reconcile for the **swarm** — enemy box figures keyed by stable id.
/// Figures persist across frames; only `Transform` (position + yaw) and the
/// limb walk-swing update. New ids spawn a figure; departed ids despawn theirs.
/// Runs after the tick + camera-follow so it reads post-tick state.
pub fn sync_enemies(
    mut commands: Commands,
    sim: Res<Sim>,
    content: Res<GameContent>,
    assets: Res<RenderAssets>,
    mut cache: ResMut<MatCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut parents: Query<(Entity, &EnemyView, &mut Transform, &mut WalkCycle, &Children)>,
    mut limbs: Query<(&mut Transform, &Limb), (Without<EnemyView>, Without<PlayerView>)>,
) {
    let world = &sim.0;
    let live: HashMap<u64, &EnemyInstance> = world.enemies.iter().map(|e| (e.id, e)).collect();

    // Update surviving figures in place; despawn those whose enemy is gone.
    let mut seen: HashSet<u64> = HashSet::default();
    for (ent, view, mut tf, mut cycle, children) in &mut parents {
        let Some(e) = live.get(&view.0) else {
            commands.entity(ent).despawn();
            continue;
        };
        seen.insert(view.0);
        *tf = Transform::from_xyz(e.x, 0.0, e.y).with_rotation(Quat::from_rotation_y(e.facing));
        let swing = advance_walk(&mut cycle, Vec2::new(e.x, e.y));
        for &c in children {
            if let Ok((mut ct, limb)) = limbs.get_mut(c) {
                ct.translation.z = limb.amp_z * swing;
            }
        }
    }

    // Spawn figures for enemies that don't have one yet.
    let mut painter = Painter {
        cache: &mut cache,
        materials: &mut materials,
    };
    for e in &world.enemies {
        if seen.contains(&e.id) {
            continue;
        }
        let id = content
            .0
            .enemies
            .get(e.archetype)
            .map(|a| a.id.as_str())
            .unwrap_or("");
        let (color, scale, width) = enemy_shape(id);
        let spec = FigureSpec {
            scale,
            width,
            body: color,
            head: lighten(color, 0.12),
            accent: Color::srgb(0.90, 0.96, 0.45),
            phase: 0.0,
            gun: false,
        };
        let ent = spawn_figure(
            &mut commands,
            &assets,
            &mut painter,
            Vec3::new(e.x, 0.0, e.y),
            e.facing,
            &spec,
        );
        commands.entity(ent).insert((
            EnemyView(e.id),
            WalkCycle {
                phase: 0.0,
                last: Vec2::new(e.x, e.y),
            },
        ));
    }
}

/// Pooled reconcile for projectiles (swarm-heavy). Reuses a `Vec<Entity>` by
/// index — grown/shrunk only on count change, positions + material (normal vs
/// rocket) updated in place. No stable id needed: the visual is positional.
pub fn sync_projectiles(
    mut commands: Commands,
    sim: Res<Sim>,
    assets: Res<RenderAssets>,
    mut cache: ResMut<MatCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut pool: ResMut<ProjectilePool>,
    mut q: Query<(&mut Transform, &mut MeshMaterial3d<StandardMaterial>), With<ProjectileMarker>>,
) {
    let world = &sim.0;
    let mut painter = Painter {
        cache: &mut cache,
        materials: &mut materials,
    };
    for (i, p) in world.projectiles.iter().enumerate() {
        let (size, color) = if p.aoe_radius > 0.0 {
            (0.38, Color::srgb(1.0, 0.5, 0.2)) // rocket: chunkier, hotter
        } else {
            (0.22, Color::srgb(1.0, 0.9, 0.4))
        };
        let mat = painter.mat(color);
        let transform = Transform::from_xyz(p.x, 0.5, p.y).with_scale(Vec3::splat(size));
        if i < pool.0.len() {
            // Pre-existing (spawned a prior frame) — update in place. Only
            // rewrite the material handle when the color actually changed (a
            // pooled slot flipping between bullet and rocket), so an unchanged
            // projectile doesn't trip Bevy's `Changed` material detection.
            if let Ok((mut tf, mut mm)) = q.get_mut(pool.0[i]) {
                *tf = transform;
                if mm.0 != mat {
                    mm.0 = mat;
                }
            }
        } else {
            let ent = commands
                .spawn((
                    Mesh3d(assets.cube.clone()),
                    MeshMaterial3d(mat),
                    transform,
                    ProjectileMarker,
                ))
                .id();
            pool.0.push(ent);
        }
    }
    // Retire surplus pool entities when the projectile count drops.
    while pool.0.len() > world.projectiles.len() {
        let ent = pool.0.pop().expect("len checked > 0");
        commands.entity(ent).despawn();
    }
}

/// Naive reconcile for loot drops — the one remaining cheap, low-churn dynamic
/// (a handful on the ground, changing only on pickup/spawn). Despawn-all +
/// respawn every frame; negligible. The player figure, health bars, and
/// explosions used to ride here too but are now pooled/persistent (see
/// [`sync_player`] / [`sync_health_bars`] / [`spawn_explosions`]).
pub fn sync_misc(
    mut commands: Commands,
    sim: Res<Sim>,
    assets: Res<RenderAssets>,
    mut cache: ResMut<MatCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    existing: Query<Entity, With<DynamicEntity>>,
) {
    for e in &existing {
        commands.entity(e).despawn();
    }

    let mut painter = Painter {
        cache: &mut cache,
        materials: &mut materials,
    };

    // Drops (cube + rarity loot beam).
    for d in &sim.0.drops {
        spawn_drop(&mut commands, &assets, &mut painter, d);
    }
}

/// Pooled reconcile for the single player figure: spawn the ~11-cube hierarchy
/// once, then only update its root `Transform` (position + aim yaw) and limb
/// swing each frame — the same in-place pattern the swarm uses, replacing the
/// old despawn-all/respawn that rebuilt the whole figure every frame.
pub fn sync_player(
    mut commands: Commands,
    sim: Res<Sim>,
    assets: Res<RenderAssets>,
    aim: Res<crate::Aim>,
    mut cache: ResMut<MatCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut fig: Query<(&mut Transform, &mut WalkCycle, &Children), With<PlayerView>>,
    mut limbs: Query<(&mut Transform, &Limb), (Without<PlayerView>, Without<EnemyView>)>,
) {
    let p = &sim.0.player;
    if let Ok((mut tf, mut cycle, children)) = fig.single_mut() {
        *tf = Transform::from_xyz(p.x, 0.0, p.y).with_rotation(Quat::from_rotation_y(aim.yaw));
        let swing = advance_walk(&mut cycle, Vec2::new(p.x, p.y));
        for &c in children {
            if let Ok((mut ct, limb)) = limbs.get_mut(c) {
                ct.translation.z = limb.amp_z * swing;
            }
        }
        return;
    }

    // First frame (or after a restart cleared it): build the figure once.
    let mut painter = Painter {
        cache: &mut cache,
        materials: &mut materials,
    };
    let spec = FigureSpec {
        scale: PLAYER_HALF,
        width: 1.0,
        body: Color::srgb(0.32, 0.72, 0.38),
        head: Color::srgb(0.40, 0.85, 0.46),
        accent: Color::srgb(0.85, 1.00, 0.55),
        phase: 0.0,
        gun: true,
    };
    let ent = spawn_figure(
        &mut commands,
        &assets,
        &mut painter,
        Vec3::new(p.x, 0.0, p.y),
        aim.yaw,
        &spec,
    );
    commands.entity(ent).insert((
        PlayerView,
        WalkCycle {
            phase: 0.0,
            last: Vec2::new(p.x, p.y),
        },
    ));
}

/// Pooled reconcile for enemy health bars, keyed by enemy id. A bar persists
/// (parent + backing + fill) while its enemy is damaged; only the fill child's
/// width and color update each frame. Despawns when the enemy heals to full or
/// dies. The fill owns a unique material so its animated color is mutated in
/// place — never routed through the shared color cache (which would leak a fresh
/// material per distinct HP fraction).
pub fn sync_health_bars(
    mut commands: Commands,
    sim: Res<Sim>,
    content: Res<GameContent>,
    assets: Res<RenderAssets>,
    mut cache: ResMut<MatCache>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut bars: Query<(Entity, &HealthBarView, &mut Transform, &Children)>,
    mut fills: Query<
        (&mut Transform, &MeshMaterial3d<StandardMaterial>),
        (With<HealthBarFill>, Without<HealthBarView>),
    >,
) {
    // Bar geometry for every currently-damaged enemy, keyed by id.
    let want: HashMap<u64, HealthBar> = sim
        .0
        .enemies
        .iter()
        .filter_map(|e| health_bar(&content, e).map(|b| (e.id, b)))
        .collect();

    // Update surviving bars in place; despawn those whose enemy healed or died.
    let mut seen: HashSet<u64> = HashSet::default();
    for (ent, view, mut tf, children) in &mut bars {
        let Some(b) = want.get(&view.0) else {
            commands.entity(ent).despawn();
            continue;
        };
        seen.insert(view.0);
        *tf = Transform::from_xyz(b.x, b.bar_y, b.y);
        for &c in children {
            if let Ok((mut ft, mm)) = fills.get_mut(c) {
                ft.translation.x = -b.bar_w * 0.5 + b.fill_w * 0.5;
                ft.scale.x = b.fill_w;
                if let Some(mut m) = materials.get_mut(&mm.0) {
                    m.base_color = b.fill_color;
                }
            }
        }
    }

    // Spawn bars for newly-damaged enemies.
    let mut painter = Painter {
        cache: &mut cache,
        materials: &mut materials,
    };
    for (id, b) in &want {
        if seen.contains(id) {
            continue;
        }
        spawn_health_bar(&mut commands, &assets, &mut painter, *id, b);
    }
}

/// Drain this frame's `ExplosionEvent`s and spawn a self-culling blast sphere
/// for each — the sim emits, the render layer owns the visual + its clock. Each
/// gets a unique fading material (freed with the entity on despawn), so nothing
/// leaks into the shared cache.
pub fn spawn_explosions(
    mut commands: Commands,
    mut sim: ResMut<Sim>,
    assets: Res<RenderAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for ev in sim.0.explosion_events.drain(..) {
        let mat = materials.add(StandardMaterial {
            base_color: Color::srgba(1.0, 0.55, 0.15, 0.30),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        });
        // Starts small (progress 0 → 0.35× radius) and expands as it fades.
        let r = ev.radius * 0.35;
        commands.spawn((
            Mesh3d(assets.sphere.clone()),
            MeshMaterial3d(mat),
            Transform::from_xyz(ev.x, 0.4, ev.y).with_scale(Vec3::splat(r * 2.0)),
            ExplosionEffect {
                ttl: EXPLOSION_TTL,
                max_ttl: EXPLOSION_TTL,
                radius: ev.radius,
            },
        ));
    }
}

/// Advance each blast sphere on its own clock: expand toward full radius, fade
/// alpha to zero, and despawn when spent. Runs off `Time` (render-side),
/// independent of the sim tick.
pub fn animate_explosions(
    mut commands: Commands,
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut q: Query<(Entity, &mut ExplosionEffect, &mut Transform, &MeshMaterial3d<StandardMaterial>)>,
) {
    let dt = time.delta_secs();
    for (ent, mut fx, mut tf, mm) in &mut q {
        fx.ttl -= dt;
        if fx.ttl <= 0.0 {
            commands.entity(ent).despawn();
            continue;
        }
        let frac = (fx.ttl / fx.max_ttl).clamp(0.0, 1.0); // 1 at impact → 0 at cull
        let progress = 1.0 - frac;
        let r = fx.radius * (0.35 + 0.65 * progress);
        tf.scale = Vec3::splat(r * 2.0);
        if let Some(mut m) = materials.get_mut(&mm.0) {
            m.base_color = Color::srgba(1.0, 0.55, 0.15, 0.30 * frac);
        }
    }
}

/// Immediate-mode aim line + marker at the ground hit (from player to cursor).
pub fn draw_aim(sim: Res<Sim>, aim: Res<crate::Aim>, mut gizmos: Gizmos) {
    let Some(hit) = aim.hit else {
        return;
    };
    let p = &sim.0.player;
    gizmos.line(
        Vec3::new(p.x, 0.25, p.y),
        Vec3::new(hit.x, 0.05, hit.z),
        Color::srgba(0.6, 0.6, 0.65, 0.5),
    );
    gizmos.cube(
        Transform::from_xyz(hit.x, 0.1, hit.z).with_scale(Vec3::new(0.3, 0.2, 0.3)),
        Color::srgba(0.9, 0.3, 0.3, 0.9),
    );
}

/// A boxy humanoid built from stacked cube parts. `ground` = feet position,
/// `yaw` faces the heading (front = local +Z). Parent carries the yaw; children
/// are local-space so world-aligned overlays (health bars) stay outside it.
struct FigureSpec {
    scale: f32,
    width: f32,
    body: Color,
    head: Color,
    accent: Color,
    phase: f32,
    gun: bool,
}

/// Spawn a box figure and return its parent entity (caller tags it — `EnemyView`
/// for pooled enemies, `DynamicEntity` for the naive player). Legs/arms carry a
/// [`Limb`] so a pooled figure can walk-bob without respawning; the initial
/// swing is baked from `f.phase` for figures that never animate (the player,
/// which respawns each frame anyway).
fn spawn_figure(
    commands: &mut Commands,
    assets: &RenderAssets,
    painter: &mut Painter,
    ground: Vec3,
    yaw: f32,
    f: &FigureSpec,
) -> Entity {
    let s = f.scale;
    let w = f.width;
    let swing = f.phase.sin();
    let body = painter.mat(f.body);
    let head = painter.mat(f.head);
    let accent = painter.mat(f.accent);
    let gun_mat = painter.mat(Color::srgb(0.12, 0.12, 0.14));
    let cube = &assets.cube;

    // Precompute part transforms (local space, relative to feet at y=0).
    let leg = Vec3::new(0.42 * s, 0.75 * s, 0.5 * s);
    let leg_y = 0.375 * s;
    let torso = Vec3::new(1.0 * s * w, 0.9 * s, 0.62 * s);
    let torso_y = 0.75 * s + 0.45 * s;
    let arm = Vec3::new(0.28 * s, 0.82 * s, 0.4 * s);
    let arm_x = 0.5 * s * w + 0.2 * s;
    let head_sz = Vec3::new(0.64 * s, 0.6 * s, 0.62 * s);
    let head_y = 0.75 * s + 0.9 * s + 0.3 * s;
    let eye = Vec3::new(0.14 * s, 0.14 * s, 0.06 * s);
    let eye_z = 0.31 * s + 0.02;
    let leg_amp = 0.18 * s;
    let arm_amp = 0.2 * s;

    commands
        .spawn((
            Transform::from_translation(ground).with_rotation(Quat::from_rotation_y(yaw)),
            Visibility::default(),
        ))
        .with_children(|p| {
            let mut cuboid = |t: Vec3, size: Vec3, mat: &Handle<StandardMaterial>| {
                p.spawn((
                    Mesh3d(cube.clone()),
                    MeshMaterial3d(mat.clone()),
                    Transform::from_translation(t).with_scale(size),
                ));
            };
            // Head + eyes (accent on the +Z front, showing the heading).
            cuboid(Vec3::new(0.0, head_y, 0.0), head_sz, &head);
            cuboid(Vec3::new(-0.16 * s, head_y + 0.06 * s, eye_z), eye, &accent);
            cuboid(Vec3::new(0.16 * s, head_y + 0.06 * s, eye_z), eye, &accent);
            // Torso.
            cuboid(Vec3::new(0.0, torso_y, 0.0), torso, &body);
            // Gun: a dark barrel jutting forward from the right hand.
            if f.gun {
                cuboid(
                    Vec3::new(arm_x, torso_y - 0.05 * s, 0.4 * s),
                    Vec3::new(0.16 * s, 0.16 * s, 0.9 * s),
                    &gun_mat,
                );
            }
            // Legs + arms carry a Limb so the walk-bob can animate them in place.
            // Legs shuffle fore/aft in antiphase; arms swing opposite the legs.
            let mut limb = |t: Vec3, size: Vec3, amp_z: f32| {
                p.spawn((
                    Mesh3d(cube.clone()),
                    MeshMaterial3d(body.clone()),
                    Transform::from_translation(t).with_scale(size),
                    Limb { amp_z },
                ));
            };
            limb(Vec3::new(-0.30 * s, leg_y, leg_amp * swing), leg, leg_amp);
            limb(Vec3::new(0.30 * s, leg_y, -leg_amp * swing), leg, -leg_amp);
            limb(Vec3::new(-arm_x, torso_y - 0.02 * s, -arm_amp * swing), arm, -arm_amp);
            limb(Vec3::new(arm_x, torso_y - 0.02 * s, arm_amp * swing), arm, arm_amp);
        })
        .id()
}

/// Fill-bar thickness (height, depth) in world units — the backing pads this.
const HEALTH_BAR_H: f32 = 0.1;
const HEALTH_BAR_D: f32 = 0.06;

/// Computed world-space geometry + fill color for an enemy's health bar this
/// frame. Position is the enemy's anchor; the fill's local offset/width derive
/// from `bar_w` and `fill_w`.
struct HealthBar {
    x: f32,
    y: f32,
    bar_y: f32,
    bar_w: f32,
    fill_w: f32,
    fill_color: Color,
}

/// Health-bar geometry for `e`, or `None` if it's at full life (or lifeless) and
/// should show no bar — keeps a fresh swarm clean.
fn health_bar(content: &GameContent, e: &EnemyInstance) -> Option<HealthBar> {
    let max = e.combatant.max_life;
    let cur = e.combatant.current_life;
    if max <= 0.0 || cur >= max {
        return None;
    }
    let id = content
        .0
        .enemies
        .get(e.archetype)
        .map(|a| a.id.as_str())
        .unwrap_or("");
    let (_, scale, _) = enemy_shape(id);
    let frac = (cur / max).clamp(0.0, 1.0);
    let bar_w = (scale * 2.4).max(0.5);
    Some(HealthBar {
        x: e.x,
        y: e.y,
        bar_y: figure_top(scale) + 0.16,
        bar_w,
        fill_w: (bar_w * frac).max(0.001),
        fill_color: Color::srgb((1.0 - frac).min(1.0), frac.min(1.0), 0.15),
    })
}

/// Spawn a pooled, world-aligned health bar (parent [`HealthBarView`] + a fixed
/// dark backing + a green→red fill) for enemy `id`. The fill gets its **own**
/// unique material so [`sync_health_bars`] can mutate its color in place without
/// leaking a cached material per HP fraction. Only the fill moves after spawn.
fn spawn_health_bar(
    commands: &mut Commands,
    assets: &RenderAssets,
    painter: &mut Painter,
    id: u64,
    b: &HealthBar,
) {
    let back = painter.mat(Color::srgb(0.05, 0.05, 0.06));
    let fill = painter.materials.add(StandardMaterial {
        base_color: b.fill_color,
        unlit: true,
        ..default()
    });
    commands
        .spawn((
            Transform::from_xyz(b.x, b.bar_y, b.y),
            Visibility::default(),
            HealthBarView(id),
        ))
        .with_children(|p| {
            // Backing (fixed size, never updated).
            p.spawn((
                Mesh3d(assets.cube.clone()),
                MeshMaterial3d(back),
                Transform::from_scale(Vec3::new(b.bar_w + 0.04, HEALTH_BAR_H + 0.03, HEALTH_BAR_D)),
            ));
            // Fill (width / x-offset / color updated in place each frame).
            p.spawn((
                Mesh3d(assets.cube.clone()),
                MeshMaterial3d(fill),
                Transform::from_xyz(-b.bar_w * 0.5 + b.fill_w * 0.5, 0.0, 0.04)
                    .with_scale(Vec3::new(b.fill_w, HEALTH_BAR_H, HEALTH_BAR_D)),
                HealthBarFill,
            ));
        });
}

fn spawn_drop(commands: &mut Commands, assets: &RenderAssets, painter: &mut Painter, d: &LootDrop) {
    let color = rarity_color(d.item.rarity);
    let mat = painter.mat(color);
    commands.spawn((
        Mesh3d(assets.cube.clone()),
        MeshMaterial3d(mat.clone()),
        Transform::from_xyz(d.x, 0.18, d.y).with_scale(Vec3::splat(0.3)),
        DynamicEntity,
    ));
    // Loot beam: a thin tall cube, brighter rarities stand taller.
    let beam_top = 1.0 + d.item.rarity.index() as f32 * 0.6;
    commands.spawn((
        Mesh3d(assets.cube.clone()),
        MeshMaterial3d(mat),
        Transform::from_xyz(d.x, beam_top * 0.5, d.y).with_scale(Vec3::new(0.05, beam_top, 0.05)),
        DynamicEntity,
    ));
}

// ---- Pure helpers (shape / color / hash) --------------------------------

/// (color, scale, torso-width) per enemy archetype.
fn enemy_shape(id: &str) -> (Color, f32, f32) {
    match id {
        "swarm_rusher" => (Color::srgb(0.80, 0.50, 0.30), 0.28, 0.85),
        "fast_zombie" => (Color::srgb(0.90, 0.70, 0.20), 0.30, 0.8),
        "spitter" => (Color::srgb(0.50, 0.80, 0.40), 0.34, 1.0),
        "basic_zombie" => (Color::srgb(0.75, 0.25, 0.25), 0.38, 1.0),
        "fat_zombie" => (Color::srgb(0.55, 0.20, 0.50), 0.52, 1.7),
        "patient_zero" => (Color::srgb(0.95, 0.10, 0.10), 0.80, 1.4),
        _ => (Color::srgb(0.75, 0.25, 0.25), 0.38, 1.0),
    }
}

pub(crate) fn rarity_color(r: Rarity) -> Color {
    match r {
        Rarity::Basic => Color::srgb(0.70, 0.70, 0.70),
        Rarity::Common => Color::srgb(0.45, 0.70, 1.00),
        Rarity::Rare => Color::srgb(1.00, 0.85, 0.20),
        Rarity::Epic => Color::srgb(0.70, 0.30, 1.00),
        Rarity::Legendary => Color::srgb(1.00, 0.50, 0.10),
    }
}

/// Lighten a color toward white by `amt` (0..1) — head vs body.
fn lighten(c: Color, amt: f32) -> Color {
    let s = c.to_srgba();
    Color::srgb(
        s.red + (1.0 - s.red) * amt,
        s.green + (1.0 - s.green) * amt,
        s.blue + (1.0 - s.blue) * amt,
    )
}

/// Approximate top of a figure's head for `scale` — where world-aligned
/// overlays anchor above it.
fn figure_top(scale: f32) -> f32 {
    scale * 2.3
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

/// Build the 16×16 grayscale grain image (nearest-sampled) that tints the
/// floor tiles — grit without a flat-color look. Ported from `make_floor_texture`.
fn make_floor_image() -> Image {
    const N: usize = 16;
    let mut bytes = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let h = tile_hash(x as u32, y as u32, 0xF100_0F1E);
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
    let mut img = Image::new(
        Extent3d {
            width: N as u32,
            height: N as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        bytes,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::nearest();
    img
}
