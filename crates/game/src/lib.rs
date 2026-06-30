//! Game-state primitives for the runtime layer. **No rendering or input
//! plumbing in here** — that lives in `main.rs` next door, behind macroquad.
//! Keeping the state/logic side library-shaped means:
//!
//! 1. It can be unit-tested without spinning up a window.
//! 2. It can later run server-side inside a SpacetimeDB reducer module
//!    unchanged (CLAUDE.md multiplayer posture, habit #3).
//! 3. The runtime layer never gets to mutate state directly — input is
//!    funneled through `Command` variants which are trivially serializable
//!    (habit #1).
//!
//! ## Determinism / netcode posture
//!
//! Per CLAUDE.md: "every drop reproducible from `(world_seed, event_id)`."
//! We hold `world_seed` + a monotonic `event_seq` and seed a **fresh RNG per
//! event** (each spawn batch, each hit, each loot roll) rather than threading
//! one long-lived stream. That maps cleanly onto the coop branch where each
//! event becomes an independent reducer call — no shared stream position to
//! reconstruct. Every spawned enemy and dropped item also carries a stable
//! `u64` id: the table primary key server-side, and the match key a client
//! needs to interpolate an entity across position snapshots. Projectiles get
//! no persistent id — they're ephemeral, client-replayed from their spawn.

use h2b_core::roll::roll_item;
use h2b_core::{
    Affix, BaseItem, Combatant, Enemy, HitResult, ItemInstance, Rarity, Weapon, resolve_hit,
};
use h2b_procgen::{FlowField, Map, Tile, pick_spawn_points};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

// ---------- tunables ----------
//
// Hardcoded for v1 — the weapon-derived ones (speed, fire rate, damage) come
// from the equipped item's aggregated stats once loot is wired into the
// player loadout. Encoded as constants for now so the values are visible and
// adjustable in one place.

/// Player movement speed in tiles per second.
pub const PLAYER_SPEED: f32 = 6.0;
/// Projectile speed in tiles per second. ~4× player speed so shots feel snappy.
pub const PROJECTILE_SPEED: f32 = 24.0;
/// Shots per second; reciprocal is the per-shot cooldown.
pub const FIRE_RATE: f32 = 4.0;
/// Damage applied on a projectile-vs-enemy hit. Folded into a one-shot
/// `Weapon` so `core::combat::resolve_hit` can apply the enemy's armor/dodge.
pub const BULLET_DAMAGE: f32 = 12.0;
/// Hard cap on how long a projectile can live before it despawns, in seconds.
pub const BULLET_LIFETIME: f32 = 1.5;

/// Player starting / max life. Invented here; becomes gear-derived once armor
/// items feed the player's aggregated stats.
pub const PLAYER_MAX_LIFE: f32 = 100.0;
/// An enemy within this distance (tiles) of the player deals contact damage.
pub const CONTACT_RANGE: f32 = 0.6;
/// Damage-per-second a touching enemy applies. Enemies have no attack profile
/// in content yet (see `core::enemy`), so melee is a flat game-layer rate for
/// now — replace with per-archetype attack stats when those land.
pub const CONTACT_DPS: f32 = 8.0;
/// How close (tiles) a projectile must pass to an enemy to count as a hit.
pub const PROJECTILE_HIT_RADIUS: f32 = 0.45;
/// How close (tiles) the player must be to a ground drop to pick it up.
pub const PICKUP_RADIUS: f32 = 0.7;
/// Seconds between spawn waves.
pub const SPAWN_INTERVAL: f32 = 3.0;
/// Enemies attempted per wave (capped by `MAX_ENEMIES`).
pub const SPAWN_BATCH: usize = 4;
/// Hard cap on concurrent live enemies.
pub const MAX_ENEMIES: usize = 40;
/// Minimum spawn distance (tiles, flow-field steps) from the player so a wave
/// never materializes on top of them.
pub const SPAWN_MIN_DISTANCE: u32 = 12;
/// Probability a kill drops an item at all. Rarity is rolled separately.
pub const DROP_CHANCE: f32 = 0.35;

/// Live, runtime-mutable copy of the gameplay knobs above. The simulation
/// reads **every** tunable value from here rather than the `const`s directly,
/// so a debug overlay (or, later, a difficulty preset) can retune the feel
/// without a recompile. The `const`s remain the single source of the *default*
/// values — [`Tunables::default`] just snapshots them — so behaviour and the
/// unit tests are unchanged until something deliberately mutates a field.
///
/// Plain `Copy` data: it's part of the serializable world state, not behind
/// any rendering or debug feature. Stripping the debug UI strips the *editor*,
/// not this struct. The serde derives are themselves debug-only (RON
/// export/import lives in the overlay) so non-debug builds pull in no serde.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "debug", derive(serde::Serialize, serde::Deserialize))]
pub struct Tunables {
    pub player_speed: f32,
    pub projectile_speed: f32,
    pub fire_rate: f32,
    pub bullet_damage: f32,
    pub bullet_lifetime: f32,
    /// Per-shot crit chance (0..=1) threaded into the projectile's `Weapon`.
    /// Defaults to `0.0` (shipping projectiles don't crit); the weapon picker
    /// in the debug overlay loads real values from a weapon base.
    pub crit_chance: f32,
    /// Crit damage multiplier (`1.5` = 150% on a crit). Paired with `crit_chance`.
    pub crit_multiplier: f32,
    pub contact_dps: f32,
    pub spawn_interval: f32,
    pub spawn_batch: usize,
    pub max_enemies: usize,
    pub spawn_min_distance: u32,
    pub drop_chance: f32,
    /// Global multiplier on every enemy's per-archetype move speed. The cheap
    /// lever for tuning swarm convergence / pathing pressure without touching
    /// the flow field — `1.0` is shipping behaviour.
    pub enemy_speed_mult: f32,
    /// Distance (tiles) at which a dormant enemy with clear line of sight spots
    /// the player and wakes (then gives chase permanently). Independent of
    /// `los_range` — sight gates *aggro*, los_range gates the *approach style*.
    pub sight_range: f32,
    /// Max distance (tiles) at which an enemy with clear line of sight beelines
    /// straight at the player (radial approach). Beyond it — or when a wall
    /// blocks the line — it paths via the flow field instead. `0` disables the
    /// beeline entirely (pure flow-field steering).
    pub los_range: f32,
    /// Boids-style separation strength: how hard enemies push apart so a swarm
    /// rings the player instead of stacking on one point. `0` disables it.
    pub separation_weight: f32,
    /// Radius (tiles) within which enemies repel one another.
    pub separation_radius: f32,
    /// Gamepad stick deadzone (0..1) — magnitude below this reads as neutral.
    /// Applies to both sticks. Client-side input config that rides along in the
    /// tunables for live tuning + preset export.
    pub stick_deadzone: f32,
    /// When false, the wave timer is suspended — useful for hand-spawning a
    /// controlled set of enemies to study one interaction at a time.
    pub auto_spawn: bool,
    /// When true, the player takes no contact damage (and can't die).
    pub god_mode: bool,
}

impl Default for Tunables {
    fn default() -> Self {
        Self {
            player_speed: PLAYER_SPEED,
            projectile_speed: PROJECTILE_SPEED,
            fire_rate: FIRE_RATE,
            bullet_damage: BULLET_DAMAGE,
            bullet_lifetime: BULLET_LIFETIME,
            crit_chance: 0.0,
            crit_multiplier: 1.0,
            contact_dps: CONTACT_DPS,
            spawn_interval: SPAWN_INTERVAL,
            spawn_batch: SPAWN_BATCH,
            max_enemies: MAX_ENEMIES,
            spawn_min_distance: SPAWN_MIN_DISTANCE,
            drop_chance: DROP_CHANCE,
            enemy_speed_mult: 1.0,
            sight_range: 12.0,
            los_range: 20.0,
            separation_weight: 1.5,
            separation_radius: 1.0,
            stick_deadzone: 0.2,
            auto_spawn: true,
            god_mode: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Player {
    /// Position in **tile coordinates**, with sub-tile precision. The
    /// renderer multiplies by tile size to get screen pixels.
    pub x: f32,
    pub y: f32,
    pub max_life: f32,
    pub current_life: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Projectile {
    pub x: f32,
    pub y: f32,
    /// Velocity in tiles per second.
    pub vx: f32,
    pub vy: f32,
    pub damage: f32,
    /// Crit chance / multiplier snapshotted from the tunables at fire time, so
    /// the hit resolves with the same `Weapon` the shot was fired with even if
    /// the player swaps weapons mid-flight.
    pub crit_chance: f32,
    pub crit_multiplier: f32,
    /// Seconds remaining before forced despawn.
    pub lifetime: f32,
}

/// A live enemy in the world. `archetype` indexes into [`Content::enemies`]
/// for the static profile (name, xp, ilvl); the mutable combat state lives in
/// `combatant`. `id` is stable for the entity's lifetime.
#[derive(Debug, Clone)]
pub struct EnemyInstance {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub archetype: usize,
    pub combatant: Combatant,
    /// Movement speed in tiles per second (per-archetype, see [`archetype_speed`]).
    pub speed: f32,
    /// Aggro latch. Spawns `false` (inert — holds position); flips `true` once
    /// the enemy spots the player (within `sight_range` + clear line of sight)
    /// or takes a hit, and stays `true` thereafter (it gives chase via the
    /// flow field even after losing sight).
    pub awake: bool,
}

/// An item lying on the ground waiting to be walked over.
#[derive(Debug, Clone)]
pub struct LootDrop {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub item: ItemInstance,
}

/// Static, loaded-once content the runtime reads but never mutates. Passed by
/// reference into [`World::tick`] so the dynamic `World` stays purely
/// serializable game state (the eventual SpacetimeDB table shape) with no
/// content baggage. Loaded from RON in `main.rs`; `core`/the lib stay IO-free.
#[derive(Debug, Clone, Default)]
pub struct Content {
    pub enemies: Vec<Enemy>,
    pub bases: Vec<BaseItem>,
    pub affixes: Vec<Affix>,
}

impl Content {
    /// Empty content — no enemies spawn, no loot rolls. Handy for tests that
    /// only exercise movement / projectiles.
    pub fn empty() -> Self {
        Self::default()
    }
}

pub struct World {
    pub map: Map,
    pub player: Player,
    pub projectiles: Vec<Projectile>,
    pub enemies: Vec<EnemyInstance>,
    pub drops: Vec<LootDrop>,
    /// Items the player has walked over and picked up. No equip/stash UI yet —
    /// pickups just accumulate here so the loot loop is observable.
    pub inventory: Vec<ItemInstance>,
    /// Seconds until the player's next shot becomes available.
    pub player_fire_cooldown: f32,
    pub kills: u32,
    pub xp: u64,
    /// Display string for the most recent pickup (HUD feedback). `None` until
    /// the first item is collected.
    pub last_pickup: Option<String>,
    /// Set once the player's life hits zero. `tick` becomes a no-op; the
    /// runtime layer decides whether to restart.
    pub game_over: bool,

    /// Runtime gameplay knobs. Mutated by the debug overlay; otherwise holds
    /// [`Tunables::default`] (i.e. the shipping `const` values).
    pub tunables: Tunables,
    /// Outcome of the most recent projectile→enemy hit (dodge / damage / crit).
    /// Discarded by gameplay; surfaced by the debug overlay so the effect of
    /// damage and enemy-armor tuning is visible per shot.
    pub last_hit: Option<HitResult>,

    // --- determinism / event sourcing (not part of the public render view) ---
    world_seed: u64,
    event_seq: u64,
    next_entity_id: u64,

    // --- pathing ---
    flow: FlowField,
    /// The player tile the current `flow` was computed for. Recomputed only
    /// when the player crosses into a new tile (CLAUDE.md: once per
    /// player-tile-change, not per frame).
    flow_goal: (u32, u32),

    spawn_timer: f32,
}

impl World {
    pub fn new(map: Map) -> Self {
        let (sx, sy) = map.player_spawn;
        let goal = (sx, sy);
        let world_seed = map.seed;
        let flow = FlowField::compute(&map, goal);
        Self {
            player: Player {
                x: sx as f32 + 0.5,
                y: sy as f32 + 0.5,
                max_life: PLAYER_MAX_LIFE,
                current_life: PLAYER_MAX_LIFE,
            },
            projectiles: Vec::new(),
            enemies: Vec::new(),
            drops: Vec::new(),
            inventory: Vec::new(),
            player_fire_cooldown: 0.0,
            kills: 0,
            xp: 0,
            last_pickup: None,
            game_over: false,
            tunables: Tunables::default(),
            last_hit: None,
            world_seed,
            event_seq: 0,
            next_entity_id: 0,
            flow,
            flow_goal: goal,
            spawn_timer: SPAWN_INTERVAL,
            map,
        }
    }

    /// A fresh RNG keyed on `(world_seed, event_seq)`, advancing the event
    /// counter. SplitMix64-style avalanche so adjacent event ids don't yield
    /// correlated streams. This is the reproducibility seam: any single event
    /// (a spawn wave, a hit, a drop) replays from its `(world_seed, event_id)`.
    fn next_event_rng(&mut self) -> StdRng {
        let id = self.event_seq;
        self.event_seq += 1;
        let mut z = self.world_seed ^ id.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        StdRng::seed_from_u64(z)
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_entity_id;
        self.next_entity_id += 1;
        id
    }
}

/// Commands are the **only** way to mutate the world. Input collectors emit
/// these; tests pass them in directly. Designed to serialize cleanly so the
/// same command stream can drive a coop reducer when that lands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// `(dx, dy)` normalized direction. World scales by `dt × PLAYER_SPEED`.
    Move { dx: f32, dy: f32 },
    /// Fire in normalized direction `(dx, dy)`. No-op if the cooldown isn't
    /// up — callers can spam this every frame while LMB is held; the World
    /// rate-limits.
    Fire { dx: f32, dy: f32 },
}

impl World {
    /// Apply one command. Movement uses **per-axis collision** so the player
    /// slides along walls instead of getting hard-stopped on diagonals. Fire
    /// respects `player_fire_cooldown` and silently drops if not ready.
    /// No-op once `game_over` is set.
    pub fn apply(&mut self, cmd: Command, dt: f32) {
        if self.game_over {
            return;
        }
        match cmd {
            Command::Move { dx, dy } => {
                let step = self.tunables.player_speed * dt;
                let nx = self.player.x + dx * step;
                if self.can_stand(nx, self.player.y) {
                    self.player.x = nx;
                }
                let ny = self.player.y + dy * step;
                if self.can_stand(self.player.x, ny) {
                    self.player.y = ny;
                }
            }
            Command::Fire { dx, dy } => {
                if self.player_fire_cooldown > 0.0 {
                    return;
                }
                // Normalize defensively — caller is expected to send unit
                // vectors but a zero-vector fire would create stationary
                // projectiles, which is silly.
                let len = (dx * dx + dy * dy).sqrt();
                if len < 1e-6 {
                    return;
                }
                let (nx, ny) = (dx / len, dy / len);
                self.projectiles.push(Projectile {
                    x: self.player.x,
                    y: self.player.y,
                    vx: nx * self.tunables.projectile_speed,
                    vy: ny * self.tunables.projectile_speed,
                    damage: self.tunables.bullet_damage,
                    crit_chance: self.tunables.crit_chance,
                    crit_multiplier: self.tunables.crit_multiplier,
                    lifetime: self.tunables.bullet_lifetime,
                });
                self.player_fire_cooldown = 1.0 / self.tunables.fire_rate;
            }
        }
    }

    /// Advance time-based state by `dt` seconds. Apply this **after** command
    /// resolution each frame so newly-fired projectiles get one frame of
    /// motion before the next render. Order in main: collect → apply → tick →
    /// draw.
    ///
    /// `content` supplies enemy archetypes and the loot tables; pass
    /// [`Content::empty`] to run pure movement/projectile simulation.
    pub fn tick(&mut self, dt: f32, content: &Content) {
        if self.game_over {
            return;
        }
        self.player_fire_cooldown = (self.player_fire_cooldown - dt).max(0.0);

        self.update_flow();
        self.resolve_projectiles(dt, content);
        self.move_enemies(dt);
        self.apply_contact_damage(dt);
        self.collect_pickups();
        self.update_spawning(dt, content);

        if self.player.current_life <= 0.0 {
            self.game_over = true;
        }
    }

    /// Recompute the flow field only when the player crosses a tile boundary.
    fn update_flow(&mut self) {
        let tile = (self.player.x as u32, self.player.y as u32);
        if tile != self.flow_goal {
            self.flow = FlowField::compute(&self.map, tile);
            self.flow_goal = tile;
        }
    }

    /// Advance projectiles; despawn on enemy hit, wall hit, OOB, or lifetime
    /// expiry. On an enemy hit, damage routes through `core::combat::resolve_hit`
    /// (so enemy armor/evasion applies) and a kill may roll a drop. `swap_remove`
    /// keeps removal O(1).
    fn resolve_projectiles(&mut self, dt: f32, content: &Content) {
        let mut i = 0;
        while i < self.projectiles.len() {
            let p = self.projectiles[i];
            let new_life = p.lifetime - dt;
            if new_life <= 0.0 {
                self.projectiles.swap_remove(i);
                continue;
            }
            let nx = p.x + p.vx * dt;
            let ny = p.y + p.vy * dt;

            if let Some(ei) = self.enemy_at(nx, ny) {
                let weapon = Weapon {
                    damage_per_shot: p.damage,
                    fire_rate: 0.0,
                    crit_chance: p.crit_chance,
                    crit_multiplier: p.crit_multiplier,
                };
                let mut rng = self.next_event_rng();
                self.last_hit = Some(resolve_hit(
                    &mut rng,
                    &weapon,
                    &mut self.enemies[ei].combatant,
                ));
                // Getting shot wakes a dormant enemy.
                self.enemies[ei].awake = true;
                self.projectiles.swap_remove(i);
                if self.enemies[ei].combatant.current_life <= 0.0 {
                    self.kill_enemy(ei, content);
                }
                continue;
            }

            if !self.is_passable(nx, ny) {
                self.projectiles.swap_remove(i);
                continue;
            }

            let slot = &mut self.projectiles[i];
            slot.x = nx;
            slot.y = ny;
            slot.lifetime = new_life;
            i += 1;
        }
    }

    /// Index of the first enemy whose center is within `PROJECTILE_HIT_RADIUS`
    /// of `(x, y)`, if any.
    fn enemy_at(&self, x: f32, y: f32) -> Option<usize> {
        let r2 = PROJECTILE_HIT_RADIUS * PROJECTILE_HIT_RADIUS;
        self.enemies.iter().position(|e| {
            let dx = e.x - x;
            let dy = e.y - y;
            dx * dx + dy * dy <= r2
        })
    }

    /// Remove a dead enemy, award XP, and roll a possible loot drop at its
    /// position. The drop roll is one event keyed on `(world_seed, event_id)`.
    fn kill_enemy(&mut self, ei: usize, content: &Content) {
        let dead = self.enemies.swap_remove(ei);
        self.kills += 1;

        let (xp_value, ilvl) = match content.enemies.get(dead.archetype) {
            Some(a) => (a.xp_value as u64, a.ilvl),
            None => (0, 1),
        };
        self.xp += xp_value;

        if content.bases.is_empty() {
            return;
        }
        let mut rng = self.next_event_rng();
        if rng.r#gen::<f32>() >= self.tunables.drop_chance {
            return;
        }
        let rarity = roll_rarity(&mut rng);
        if let Ok(item) = roll_item(&mut rng, &content.bases, &content.affixes, ilvl, rarity) {
            let id = self.alloc_id();
            self.drops.push(LootDrop {
                id,
                x: dead.x,
                y: dead.y,
                item,
            });
        }
    }

    /// Move every enemy toward the player, blending a **goal-seek** direction
    /// with **boids separation** so the swarm rings the player instead of
    /// funneling onto one tile.
    ///
    /// Goal-seek (`seek_dir`): if the enemy has line of sight to the player it
    /// beelines straight at them — radial from any angle, which avoids the
    /// flow field's Manhattan "approach at 45° then snap to an axis" artifact.
    /// Only when a wall blocks the line does it fall back to the smooth
    /// flow-field gradient (`steer_from`) to route around it.
    ///
    /// Two passes so separation reads a consistent snapshot: pass 1 computes
    /// each enemy's blended unit direction (all immutable reads), pass 2 applies
    /// it with per-axis wall slide.
    fn move_enemies(&mut self, dt: f32) {
        let (px, py) = (self.player.x, self.player.y);
        let speed_mult = self.tunables.enemy_speed_mult;
        let sep_weight = self.tunables.separation_weight;

        let n = self.enemies.len();

        // Awareness pass: a dormant enemy that spots the player (within sight
        // range + clear line) wakes for good. Mutates `awake`, so it's a
        // separate loop ahead of the immutable direction pass.
        let sight = self.tunables.sight_range;
        for i in 0..n {
            if self.enemies[i].awake {
                continue;
            }
            let (ex, ey) = (self.enemies[i].x, self.enemies[i].y);
            let (dx, dy) = (px - ex, py - ey);
            if dx * dx + dy * dy <= sight * sight && self.line_clear(ex, ey, px, py) {
                self.enemies[i].awake = true;
            }
        }

        let mut dirs: Vec<(f32, f32)> = Vec::with_capacity(n);
        for i in 0..n {
            // Inert enemies hold position until they wake.
            if !self.enemies[i].awake {
                dirs.push((0.0, 0.0));
                continue;
            }
            let (ex, ey) = (self.enemies[i].x, self.enemies[i].y);
            let (skx, sky) = self.seek_dir(ex, ey, px, py);
            let (spx, spy) = self.separation(i);
            let mut dx = skx + spx * sep_weight;
            let mut dy = sky + spy * sep_weight;
            let len = (dx * dx + dy * dy).sqrt();
            if len > 1e-4 {
                dx /= len;
                dy /= len;
            } else {
                // Seek and separation cancel — hold position (ring equilibrium).
                dx = 0.0;
                dy = 0.0;
            }
            dirs.push((dx, dy));
        }

        let map = &self.map;
        for (i, e) in self.enemies.iter_mut().enumerate() {
            let (dx, dy) = dirs[i];
            let step = e.speed * speed_mult * dt;
            let nx = e.x + dx * step;
            let ny = e.y + dy * step;
            if floor_at(map, nx, e.y) {
                e.x = nx;
            }
            if floor_at(map, e.x, ny) {
                e.y = ny;
            }
        }
    }

    /// Goal-seek direction (normalized) for an enemy at `(ex, ey)`: a straight
    /// beeline at the player when they're within `los_range` *and* the line
    /// between them is wall-free, else the smooth flow-field gradient (with the
    /// discrete-saddle fallback). The range check also short-circuits the LOS
    /// sampling for distant enemies.
    fn seek_dir(&self, ex: f32, ey: f32, px: f32, py: f32) -> (f32, f32) {
        let (dx, dy) = (px - ex, py - ey);
        let dist2 = dx * dx + dy * dy;
        let los = self.tunables.los_range;
        if dist2 <= los * los && dist2 > 1e-8 && self.line_clear(ex, ey, px, py) {
            let len = dist2.sqrt();
            return (dx / len, dy / len);
        }
        self.flow
            .steer_from(ex, ey)
            .or_else(|| self.flow.next_step_dir(ex, ey))
            .unwrap_or((0.0, 0.0))
    }

    /// Boids separation push (un-normalized) for enemy `i`: the summed
    /// repulsion from every other enemy within `separation_radius`, weighted
    /// linearly by proximity (closer pushes harder). Zero if nothing's near.
    fn separation(&self, i: usize) -> (f32, f32) {
        let r = self.tunables.separation_radius;
        if r <= 0.0 {
            return (0.0, 0.0);
        }
        let e = &self.enemies[i];
        let r2 = r * r;
        let (mut sx, mut sy) = (0.0, 0.0);
        for (j, o) in self.enemies.iter().enumerate() {
            if j == i {
                continue;
            }
            let dx = e.x - o.x;
            let dy = e.y - o.y;
            let d2 = dx * dx + dy * dy;
            if d2 > 1e-6 && d2 < r2 {
                let d = d2.sqrt();
                let push = (r - d) / r; // 1 at touching → 0 at the radius
                sx += dx / d * push;
                sy += dy / d * push;
            }
        }
        (sx, sy)
    }

    /// Whether the straight segment `(x0,y0)→(x1,y1)` stays on `Floor` tiles —
    /// a cheap point-sampled line-of-sight test, sampled every quarter tile.
    fn line_clear(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        let dx = x1 - x0;
        let dy = y1 - y0;
        let dist = (dx * dx + dy * dy).sqrt();
        let steps = (dist / 0.25).ceil() as i32;
        for s in 1..=steps {
            let t = s as f32 / steps as f32;
            if !floor_at(&self.map, x0 + dx * t, y0 + dy * t) {
                return false;
            }
        }
        true
    }

    /// Drain life from the player for each enemy in contact range this tick.
    fn apply_contact_damage(&mut self, dt: f32) {
        if self.tunables.god_mode {
            return;
        }
        let (px, py) = (self.player.x, self.player.y);
        let r2 = CONTACT_RANGE * CONTACT_RANGE;
        let touching = self
            .enemies
            .iter()
            .filter(|e| {
                let dx = e.x - px;
                let dy = e.y - py;
                dx * dx + dy * dy <= r2
            })
            .count();
        if touching > 0 {
            let dmg = self.tunables.contact_dps * dt * touching as f32;
            self.player.current_life = (self.player.current_life - dmg).max(0.0);
        }
    }

    /// Move any ground drops within pickup range into the inventory.
    fn collect_pickups(&mut self) {
        let (px, py) = (self.player.x, self.player.y);
        let r2 = PICKUP_RADIUS * PICKUP_RADIUS;
        let mut i = 0;
        while i < self.drops.len() {
            let dx = self.drops[i].x - px;
            let dy = self.drops[i].y - py;
            if dx * dx + dy * dy <= r2 {
                let picked = self.drops.swap_remove(i);
                self.last_pickup = Some(format!("{:?} {}", picked.item.rarity, picked.item.base));
                self.inventory.push(picked.item);
                continue;
            }
            i += 1;
        }
    }

    /// Spawn a wave on the timer, placed by `pick_spawn_points` (distance-biased,
    /// never on the player). Archetype is a uniform pick for v1 — weighted
    /// spawn tables come with biomes later.
    fn update_spawning(&mut self, dt: f32, content: &Content) {
        if content.enemies.is_empty() || !self.tunables.auto_spawn {
            return;
        }
        let interval = self.tunables.spawn_interval;
        self.spawn_timer -= dt;
        if self.spawn_timer > 0.0 || self.enemies.len() >= self.tunables.max_enemies {
            // Reset the timer even when capped, so we don't dump a burst the
            // instant headroom opens up.
            if self.spawn_timer <= 0.0 {
                self.spawn_timer = interval;
            }
            return;
        }
        self.spawn_timer = interval;

        let budget = (self.tunables.max_enemies - self.enemies.len()).min(self.tunables.spawn_batch);
        let mut rng = self.next_event_rng();
        let points = pick_spawn_points(&mut rng, &self.flow, budget, self.tunables.spawn_min_distance);
        for (tx, ty) in points {
            let idx = rng.gen_range(0..content.enemies.len());
            self.push_enemy(idx, tx as f32 + 0.5, ty as f32 + 0.5, content);
        }
    }

    /// Construct and insert one live enemy of `archetype` (index into
    /// [`Content::enemies`]) at `(x, y)`, allocating a fresh entity id. Shared
    /// by wave spawning and the debug spawn helpers.
    fn push_enemy(&mut self, archetype: usize, x: f32, y: f32, content: &Content) {
        let arch = &content.enemies[archetype];
        let id = self.alloc_id();
        self.enemies.push(EnemyInstance {
            id,
            x,
            y,
            archetype,
            combatant: arch.as_combatant(),
            speed: archetype_speed(&arch.id),
            awake: false,
        });
    }

    fn can_stand(&self, x: f32, y: f32) -> bool {
        floor_at(&self.map, x, y)
    }

    /// Whether a projectile point at `(x, y)` is in a tile a bullet can fly
    /// through. Same as `can_stand` for now — players and projectiles share
    /// the same passability rules. Split if we ever add window/pit tiles.
    fn is_passable(&self, x: f32, y: f32) -> bool {
        self.can_stand(x, y)
    }
}

/// Debug / tuning API. These bypass the wave timer and caps to set up a
/// controlled scenario — they're driven by the debug overlay, never by normal
/// gameplay input. Kept on `World` (not behind a feature) so the headless
/// crate compiles identically; only the *renderer-side* editor is feature-gated.
impl World {
    /// Spawn `count` enemies of `archetype` (index into [`Content::enemies`])
    /// on distance-biased floor tiles at least `min_distance` from the player.
    /// Clamps `archetype` to the roster; no-op if there's no enemy content.
    pub fn debug_spawn(
        &mut self,
        archetype: usize,
        count: usize,
        min_distance: u32,
        content: &Content,
    ) {
        if content.enemies.is_empty() {
            return;
        }
        let archetype = archetype.min(content.enemies.len() - 1);
        let mut rng = self.next_event_rng();
        let points = pick_spawn_points(&mut rng, &self.flow, count, min_distance);
        for (tx, ty) in points {
            self.push_enemy(archetype, tx as f32 + 0.5, ty as f32 + 0.5, content);
        }
    }

    /// Spawn one enemy of `archetype` at an explicit world position (e.g. the
    /// cursor tile). No-op if the tile isn't standable or there's no content.
    pub fn debug_spawn_at(&mut self, archetype: usize, x: f32, y: f32, content: &Content) {
        if content.enemies.is_empty() || !self.can_stand(x, y) {
            return;
        }
        let archetype = archetype.min(content.enemies.len() - 1);
        self.push_enemy(archetype, x, y, content);
    }

    /// Remove every live enemy. The wave timer still runs (gate it via
    /// `tunables.auto_spawn` for a fully static arena).
    pub fn debug_clear_enemies(&mut self) {
        self.enemies.clear();
    }

    /// Remove every uncollected ground drop.
    pub fn debug_clear_drops(&mut self) {
        self.drops.clear();
    }

    /// Wake every live enemy (aggro them all) — handy for testing chase
    /// behaviour without first walking into each one's sight line.
    pub fn debug_wake_all(&mut self) {
        for e in &mut self.enemies {
            e.awake = true;
        }
    }

    /// Restore the player to full life and lift `game_over` — revive without a
    /// full world reset, so a tuning session survives a death.
    pub fn debug_revive(&mut self) {
        self.player.current_life = self.player.max_life;
        self.game_over = false;
    }

    /// Read-only access to the current flow field, for the debug pathing
    /// visualization. The field is otherwise private (recomputed internally on
    /// player-tile change); nothing outside debug rendering should need it.
    pub fn flow(&self) -> &FlowField {
        &self.flow
    }
}

/// Whether `(x, y)` (continuous tile coords) sits on a `Floor` tile — the
/// shared passability test for the player, projectiles, and enemy steering.
/// A free fn (not a `&self` method) so callers can borrow `&Map` disjointly
/// from a `&mut self.enemies` loop.
fn floor_at(map: &Map, x: f32, y: f32) -> bool {
    x >= 0.0 && y >= 0.0 && matches!(map.tile_at(x as u32, y as u32), Tile::Floor)
}

/// Per-archetype movement speed (tiles/sec), keyed by enemy `id`. All slower
/// than `PLAYER_SPEED` (6.0) so the player can kite. Unknown ids fall back to
/// the baseline zombie pace.
pub fn archetype_speed(id: &str) -> f32 {
    match id {
        "fast_zombie" => 4.5,
        "swarm_rusher" => 4.0,
        "spitter" => 3.2,
        "basic_zombie" => 3.0,
        "patient_zero" => 2.2,
        "fat_zombie" => 1.8,
        _ => 3.0,
    }
}

/// Weighted rarity roll matching CLAUDE.md's drop curve (per 1000): Legendary
/// 5, Epic 25, Rare 90, Common 280, Basic 600. Ported from the sim's
/// `roll_rarity`; retune both together.
fn roll_rarity<R: Rng + ?Sized>(rng: &mut R) -> Rarity {
    match rng.gen_range(0..1000) {
        0..=4 => Rarity::Legendary,
        5..=29 => Rarity::Epic,
        30..=119 => Rarity::Rare,
        120..=399 => Rarity::Common,
        _ => Rarity::Basic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2b_core::Combatant;
    use h2b_procgen::{MapParams, generate_bsp};

    fn world_at_seed(seed: u64) -> World {
        World::new(generate_bsp(&MapParams {
            seed,
            ..Default::default()
        }))
    }

    fn single_floor_map() -> Map {
        // 3×3 with one Floor at (1,1).
        let w = 3;
        let h = 3;
        let mut tiles = vec![Tile::Wall; (w * h) as usize];
        tiles[(w + 1) as usize] = Tile::Floor;
        Map {
            width: w,
            height: h,
            tiles,
            rooms: vec![],
            seed: 0,
            player_spawn: (1, 1),
        }
    }

    /// A test enemy archetype with the given id and life. No defenses so a
    /// single `BULLET_DAMAGE` shot is enough to study kills.
    fn test_enemy(id: &str, max_life: f32) -> Enemy {
        Enemy {
            id: id.into(),
            name: id.into(),
            category: "zombie".into(),
            ilvl: 1,
            max_life,
            armor: 0.0,
            evasion: 0.0,
            xp_value: 10,
        }
    }

    fn dummy_item() -> ItemInstance {
        ItemInstance {
            base: "pistol".into(),
            ilvl: 1,
            rarity: Rarity::Common,
            seed: 0,
            prefixes: vec![],
            suffixes: vec![],
            upgrade_tier: 0,
            attached: vec![],
        }
    }

    // ---- movement ----

    #[test]
    fn spawn_is_on_a_floor_tile() {
        let w = world_at_seed(42);
        assert!(w.can_stand(w.player.x, w.player.y));
    }

    #[test]
    fn move_command_advances_player_on_floor() {
        let mut w = world_at_seed(42);
        let (x0, y0) = (w.player.x, w.player.y);
        w.apply(Command::Move { dx: 1.0, dy: 0.0 }, 0.1);
        assert!(w.can_stand(w.player.x, w.player.y));
        assert!((w.player.y - y0).abs() < 1e-6);
        if w.can_stand(x0 + 1.0, y0) {
            assert!(w.player.x > x0);
        }
    }

    #[test]
    fn wall_blocks_movement_on_the_blocked_axis_only() {
        let mut w = World::new(single_floor_map());
        let (x0, y0) = (w.player.x, w.player.y);
        w.apply(Command::Move { dx: 1.0, dy: 1.0 }, 1.0);
        assert!((w.player.x - x0).abs() < 1e-6);
        assert!((w.player.y - y0).abs() < 1e-6);
    }

    #[test]
    fn per_axis_collision_allows_sliding() {
        // Map with a corridor along y (a column of floor):
        //   #.#
        //   #.#
        //   #.#
        let w = 3;
        let h = 3;
        let mut tiles = vec![Tile::Wall; (w * h) as usize];
        for y in 0..h {
            tiles[(y * w + 1) as usize] = Tile::Floor;
        }
        let map = Map {
            width: w,
            height: h,
            tiles,
            rooms: vec![],
            seed: 0,
            player_spawn: (1, 1),
        };
        let mut world = World::new(map);
        // dt × PLAYER_SPEED × dx must push the new_x across the wall
        // boundary at x=2. 0.1 × 6 × 1 = 0.6 → new_x 1.5 + 0.6 = 2.1 → wall.
        world.apply(Command::Move { dx: 1.0, dy: 1.0 }, 0.1);
        assert!((world.player.x - 1.5).abs() < 1e-6, "x stays at center of corridor");
        assert!(world.player.y > 1.5, "slid down despite x being walled");
    }

    #[test]
    fn out_of_bounds_is_not_standable() {
        let w = world_at_seed(42);
        assert!(!w.can_stand(-0.1, 5.0));
        assert!(!w.can_stand(5.0, -0.1));
        assert!(!w.can_stand(w.map.width as f32 + 1.0, 5.0));
    }

    #[test]
    fn applying_same_commands_is_deterministic() {
        let mut a = world_at_seed(42);
        let mut b = world_at_seed(42);
        let cmds = [
            Command::Move { dx: 1.0, dy: 0.0 },
            Command::Move { dx: 0.0, dy: 1.0 },
            Command::Move { dx: -1.0, dy: 0.0 },
        ];
        for cmd in cmds {
            a.apply(cmd, 0.05);
            b.apply(cmd, 0.05);
        }
        assert!((a.player.x - b.player.x).abs() < 1e-6);
        assert!((a.player.y - b.player.y).abs() < 1e-6);
    }

    // ---- shooting ----

    #[test]
    fn fire_spawns_a_projectile() {
        let mut w = world_at_seed(42);
        assert_eq!(w.projectiles.len(), 0);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        assert_eq!(w.projectiles.len(), 1);
        let p = &w.projectiles[0];
        assert!((p.x - w.player.x).abs() < 1e-6);
        assert!((p.y - w.player.y).abs() < 1e-6);
        assert!((p.vx - PROJECTILE_SPEED).abs() < 1e-4);
        assert!(p.vy.abs() < 1e-4);
        assert!(p.damage > 0.0);
        assert!(w.player_fire_cooldown > 0.0);
    }

    #[test]
    fn fire_respects_cooldown() {
        let mut w = world_at_seed(42);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        // Three fires in the same frame — only the first should spawn.
        assert_eq!(w.projectiles.len(), 1);
    }

    #[test]
    fn cooldown_expires_with_tick() {
        let mut w = world_at_seed(42);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        // Tick past the cooldown window (1/FIRE_RATE = 0.25s).
        w.tick(0.30, &Content::empty());
        assert_eq!(w.player_fire_cooldown, 0.0);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        assert_eq!(w.projectiles.len(), 2);
    }

    #[test]
    fn zero_vector_fire_is_ignored() {
        let mut w = world_at_seed(42);
        w.apply(Command::Fire { dx: 0.0, dy: 0.0 }, 0.016);
        assert_eq!(w.projectiles.len(), 0);
        assert_eq!(w.player_fire_cooldown, 0.0);
    }

    #[test]
    fn projectile_advances_with_tick() {
        let mut w = world_at_seed(42);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        let x0 = w.projectiles[0].x;
        w.tick(0.05, &Content::empty());
        // Either advanced or died on a wall — but didn't sit still.
        if let Some(p) = w.projectiles.first() {
            assert!(p.x > x0, "projectile should have moved +x");
        }
    }

    #[test]
    fn projectile_despawns_on_wall_collision() {
        // 5×3 corridor, single floor row. Fire eastward from (1,1); the
        // projectile dies as soon as it reaches the east wall.
        let w = 5;
        let h = 3;
        let mut tiles = vec![Tile::Wall; (w * h) as usize];
        for x in 1..=3u32 {
            tiles[(w + x) as usize] = Tile::Floor;
        }
        let map = Map {
            width: w,
            height: h,
            tiles,
            rooms: vec![],
            seed: 0,
            player_spawn: (1, 1),
        };
        let mut world = World::new(map);
        world.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        // PROJECTILE_SPEED is 24 tiles/s; well within 0.2s a shot crosses
        // the 3-floor-wide corridor and hits the east wall.
        world.tick(0.20, &Content::empty());
        assert!(world.projectiles.is_empty(), "projectile should hit the east wall");
    }

    #[test]
    fn projectile_despawns_on_lifetime_expiry() {
        let mut w = world_at_seed(42);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        // Force expiry without any wall in the way by ticking past lifetime.
        w.tick(BULLET_LIFETIME + 0.1, &Content::empty());
        assert!(w.projectiles.is_empty());
    }

    #[test]
    fn multiple_projectiles_coexist() {
        let mut w = world_at_seed(42);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        w.tick(0.30, &Content::empty()); // cooldown clears
        w.apply(Command::Fire { dx: 0.0, dy: 1.0 }, 0.016);
        w.tick(0.30, &Content::empty());
        w.apply(Command::Fire { dx: -1.0, dy: 0.0 }, 0.016);
        // Three distinct shots in flight (assuming none have hit walls yet).
        assert!(!w.projectiles.is_empty());
    }

    // ---- enemies: spawning ----

    #[test]
    fn wave_spawns_after_the_interval() {
        let mut w = world_at_seed(42);
        let content = Content {
            enemies: vec![test_enemy("basic_zombie", 100.0)],
            ..Content::empty()
        };
        assert!(w.enemies.is_empty());
        // One tick across the spawn interval triggers a wave.
        w.tick(SPAWN_INTERVAL, &content);
        assert!(!w.enemies.is_empty(), "a wave should have spawned");
        assert!(w.enemies.len() <= SPAWN_BATCH);
        // Spawned away from the player.
        for e in &w.enemies {
            let dx = e.x - w.player.x;
            let dy = e.y - w.player.y;
            assert!((dx * dx + dy * dy).sqrt() >= SPAWN_MIN_DISTANCE as f32 - 1.0);
        }
        // Every spawned entity has a unique id.
        let mut ids: Vec<u64> = w.enemies.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), w.enemies.len());
    }

    #[test]
    fn no_spawn_without_enemy_content() {
        let mut w = world_at_seed(42);
        w.tick(SPAWN_INTERVAL * 3.0, &Content::empty());
        assert!(w.enemies.is_empty());
    }

    // ---- enemies: pathing ----

    #[test]
    fn enemy_converges_on_player() {
        let mut w = world_at_seed(42);
        // Place an enemy on the farthest reachable tile from the player.
        let (mut far_tile, mut far_d) = ((0u32, 0u32), 0u32);
        for ty in 0..w.map.height {
            for tx in 0..w.map.width {
                let d = w.flow.distance_at(tx, ty);
                if d != h2b_procgen::UNREACHABLE && d > far_d {
                    far_d = d;
                    far_tile = (tx, ty);
                }
            }
        }
        w.enemies.push(EnemyInstance {
            id: 999,
            x: far_tile.0 as f32 + 0.5,
            y: far_tile.1 as f32 + 0.5,
            archetype: 0,
            combatant: Combatant::dummy(100.0),
            speed: 4.0,
            awake: true,
        });
        let start = w.flow.distance_at(far_tile.0, far_tile.1);
        // Several ticks of pure pathing (no content needed for movement).
        for _ in 0..120 {
            w.tick(0.05, &Content::empty());
        }
        let e = &w.enemies[0];
        let now = w.flow.distance_at(e.x as u32, e.y as u32);
        assert!(now < start, "enemy should be closer to the player (flow {now} < {start})");
    }

    /// An all-floor `w×h` room with the spawn at the center — open space for
    /// movement/steering tests (no walls to route around).
    fn open_room(w: u32, h: u32) -> Map {
        Map {
            width: w,
            height: h,
            tiles: vec![Tile::Floor; (w * h) as usize],
            rooms: vec![],
            seed: 0,
            player_spawn: (w / 2, h / 2),
        }
    }

    #[test]
    fn stacked_enemies_separate() {
        let mut w = World::new(open_room(21, 21));
        // Two enemies almost on top of each other, offset along y, both left of
        // the player (so their seek directions are nearly identical and
        // separation is what spreads them).
        let (bx, by) = (w.player.x - 4.0, w.player.y);
        for (k, dy) in [0.05_f32, -0.05].into_iter().enumerate() {
            w.enemies.push(EnemyInstance {
                id: k as u64,
                x: bx,
                y: by + dy,
                archetype: 0,
                combatant: Combatant::dummy(100.0),
                speed: 3.0,
                awake: false,
            });
        }
        let gap0 = (w.enemies[0].y - w.enemies[1].y).abs();
        for _ in 0..10 {
            w.tick(0.05, &Content::empty());
        }
        let gap1 = (w.enemies[0].y - w.enemies[1].y).abs();
        assert!(gap1 > gap0, "separation should spread stacked enemies ({gap0} -> {gap1})");
    }

    #[test]
    fn separation_off_lets_them_overlap() {
        let mut w = World::new(open_room(21, 21));
        w.tunables.separation_weight = 0.0;
        let (bx, by) = (w.player.x - 4.0, w.player.y);
        for (k, dy) in [0.05_f32, -0.05].into_iter().enumerate() {
            w.enemies.push(EnemyInstance {
                id: k as u64,
                x: bx,
                y: by + dy,
                archetype: 0,
                combatant: Combatant::dummy(100.0),
                speed: 3.0,
                awake: false,
            });
        }
        let gap0 = (w.enemies[0].y - w.enemies[1].y).abs();
        for _ in 0..10 {
            w.tick(0.05, &Content::empty());
        }
        let gap1 = (w.enemies[0].y - w.enemies[1].y).abs();
        // Both beeline at the same player with no push apart — gap shouldn't grow.
        assert!(gap1 <= gap0 + 1e-3, "no separation → no spreading ({gap0} -> {gap1})");
    }

    #[test]
    fn open_space_enemy_beelines_at_player() {
        // In open space (clear line of sight) a diagonal approach should head
        // straight at the player, not down a cardinal lane — the displacement
        // stays parallel to the enemy→player vector.
        let mut w = World::new(open_room(21, 21));
        let (sx, sy) = (w.player.x - 3.0, w.player.y - 3.0);
        w.enemies.push(EnemyInstance {
            id: 1,
            x: sx,
            y: sy,
            archetype: 0,
            combatant: Combatant::dummy(100.0),
            speed: 3.0,
            awake: false,
        });
        w.tick(0.05, &Content::empty());
        let e = &w.enemies[0];
        let moved = (e.x - sx, e.y - sy);
        // A straight beeline from a 45° offset moves x and y about equally.
        assert!(moved.0 > 1e-3 && moved.1 > 1e-3, "should move on both axes");
        assert!(
            (moved.0 - moved.1).abs() < 0.2 * moved.0.max(moved.1),
            "diagonal beeline should move x≈y, got {moved:?}"
        );
    }

    #[test]
    fn los_range_gates_the_beeline() {
        // Shallow-diagonal offset (6 east, 1 north of the player). A beeline
        // moves mostly along x (small y/x); the Manhattan flow field instead
        // pulls a 45° diagonal (y ≈ x). So disabling LOS should make the
        // approach noticeably more diagonal.
        let run = |los: f32| -> f32 {
            let mut w = World::new(open_room(31, 31));
            w.tunables.los_range = los;
            w.tunables.separation_weight = 0.0;
            let (sx, sy) = (w.player.x - 6.0, w.player.y - 1.0);
            w.enemies.push(EnemyInstance {
                id: 1,
                x: sx,
                y: sy,
                archetype: 0,
                combatant: Combatant::dummy(100.0),
                speed: 3.0,
                awake: false,
            });
            w.tick(0.05, &Content::empty());
            let e = &w.enemies[0];
            (e.y - sy) / (e.x - sx) // y/x movement ratio
        };
        let beeline = run(50.0); // in range → straight at player, shallow
        let flow = run(0.0); // LOS off → flow field, ~45°
        assert!(
            flow > beeline + 0.1,
            "flow approach should be more diagonal than the beeline ({beeline} vs {flow})"
        );
    }

    #[test]
    fn enemy_stays_inert_out_of_sight() {
        // Enemy spawns dormant and the player is beyond sight range — it must
        // not move.
        let mut w = World::new(open_room(41, 41));
        w.tunables.sight_range = 5.0;
        let (sx, sy) = (w.player.x - 12.0, w.player.y); // 12 > 5: never spotted
        w.enemies.push(EnemyInstance {
            id: 1,
            x: sx,
            y: sy,
            archetype: 0,
            combatant: Combatant::dummy(100.0),
            speed: 3.0,
            awake: false,
        });
        for _ in 0..20 {
            w.tick(0.05, &Content::empty());
        }
        assert!(!w.enemies[0].awake, "should stay dormant out of sight");
        assert!(
            (w.enemies[0].x - sx).abs() < 1e-4 && (w.enemies[0].y - sy).abs() < 1e-4,
            "inert enemy should not have moved"
        );
    }

    #[test]
    fn enemy_wakes_and_chases_when_spotted() {
        let mut w = World::new(open_room(41, 41));
        w.tunables.sight_range = 20.0;
        let (sx, sy) = (w.player.x - 8.0, w.player.y); // 8 < 20, clear line
        w.enemies.push(EnemyInstance {
            id: 1,
            x: sx,
            y: sy,
            archetype: 0,
            combatant: Combatant::dummy(100.0),
            speed: 3.0,
            awake: false,
        });
        w.tick(0.1, &Content::empty());
        assert!(w.enemies[0].awake, "in-sight player should wake the enemy");
        assert!(w.enemies[0].x > sx, "woken enemy should advance on the player");
    }

    #[test]
    fn projectile_hit_wakes_a_dormant_enemy() {
        // Sight disabled, so only the shot can wake it.
        let mut w = World::new(single_floor_map());
        w.tunables.sight_range = 0.0;
        w.enemies.push(EnemyInstance {
            id: 1,
            x: w.player.x + 0.3,
            y: w.player.y,
            archetype: 0,
            combatant: Combatant::dummy(1000.0),
            speed: 0.0,
            awake: false,
        });
        assert!(!w.enemies[0].awake);
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        w.tick(0.02, &Content::empty());
        assert!(w.enemies[0].awake, "getting shot should wake the enemy");
    }

    // ---- enemies: hit detection + kills ----

    #[test]
    fn projectile_hits_and_damages_enemy() {
        let mut w = World::new(single_floor_map());
        // Enemy a hair to the +x side of the player, within the same tile.
        w.enemies.push(EnemyInstance {
            id: 1,
            x: w.player.x + 0.3,
            y: w.player.y,
            archetype: 0,
            combatant: Combatant::dummy(1000.0),
            speed: 0.0,
            awake: false,
        });
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        w.tick(0.02, &Content::empty());
        // Projectile consumed by the hit; enemy took BULLET_DAMAGE.
        assert!(w.projectiles.is_empty(), "projectile should be consumed on hit");
        assert!((w.enemies[0].combatant.current_life - (1000.0 - BULLET_DAMAGE)).abs() < 1e-3);
    }

    #[test]
    fn lethal_hit_kills_awards_xp_and_can_drop() {
        let mut w = World::new(single_floor_map());
        let content = Content {
            enemies: vec![test_enemy("basic_zombie", 1.0)],
            ..Content::empty()
        };
        w.enemies.push(EnemyInstance {
            id: 1,
            x: w.player.x + 0.3,
            y: w.player.y,
            archetype: 0,
            combatant: Combatant::dummy(1.0), // dies to one shot
            speed: 0.0,
            awake: false,
        });
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        w.tick(0.02, &content);
        assert!(w.enemies.is_empty(), "enemy should be dead");
        assert_eq!(w.kills, 1);
        assert_eq!(w.xp, 10);
        // No bases in content → no item rolled, drops stay empty.
        assert!(w.drops.is_empty());
    }

    // ---- loot pickup ----

    #[test]
    fn walking_over_a_drop_collects_it() {
        let mut w = World::new(single_floor_map());
        assert!(w.inventory.is_empty());
        w.drops.push(LootDrop {
            id: 7,
            x: w.player.x, // right on top of the player
            y: w.player.y,
            item: dummy_item(),
        });
        w.tick(0.016, &Content::empty());
        assert!(w.drops.is_empty(), "drop should be collected");
        assert_eq!(w.inventory.len(), 1);
        assert!(w.last_pickup.is_some());
    }

    #[test]
    fn distant_drop_is_not_collected() {
        let mut w = world_at_seed(42);
        w.drops.push(LootDrop {
            id: 8,
            x: w.player.x + 5.0,
            y: w.player.y,
            item: dummy_item(),
        });
        w.tick(0.016, &Content::empty());
        assert_eq!(w.drops.len(), 1);
        assert!(w.inventory.is_empty());
    }

    // ---- contact damage + game over ----

    #[test]
    fn touching_enemy_drains_life_and_can_end_the_game() {
        let mut w = World::new(single_floor_map());
        w.player.current_life = 1.0;
        w.enemies.push(EnemyInstance {
            id: 1,
            x: w.player.x,
            y: w.player.y,
            archetype: 0,
            combatant: Combatant::dummy(100.0),
            speed: 0.0,
            awake: false,
        });
        w.tick(1.0, &Content::empty()); // CONTACT_DPS × 1s = 8 dmg > 1 life
        assert!(w.player.current_life <= 0.0);
        assert!(w.game_over);
        // Once over, tick is inert.
        let cmds_before = w.kills;
        w.tick(1.0, &Content::empty());
        assert_eq!(w.kills, cmds_before);
    }

    // ---- debug / tunables ----

    #[test]
    fn tunables_default_matches_the_constants() {
        let t = Tunables::default();
        assert_eq!(t.player_speed, PLAYER_SPEED);
        assert_eq!(t.bullet_damage, BULLET_DAMAGE);
        assert_eq!(t.fire_rate, FIRE_RATE);
        assert_eq!(t.spawn_interval, SPAWN_INTERVAL);
        assert!(t.auto_spawn);
        assert!(!t.god_mode);
        assert_eq!(t.enemy_speed_mult, 1.0);
    }

    #[cfg(feature = "debug")]
    #[test]
    fn tunables_ron_round_trips() {
        let t = Tunables {
            bullet_damage: 73.5,
            enemy_speed_mult: 2.25,
            god_mode: true,
            auto_spawn: false,
            max_enemies: 123,
            ..Tunables::default()
        };
        let ron = ron::ser::to_string(&t).unwrap();
        let back: Tunables = ron::from_str(&ron).unwrap();
        assert_eq!(back.bullet_damage, t.bullet_damage);
        assert_eq!(back.enemy_speed_mult, t.enemy_speed_mult);
        assert!(back.god_mode);
        assert!(!back.auto_spawn);
        assert_eq!(back.max_enemies, 123);
    }

    #[test]
    fn god_mode_blocks_contact_damage() {
        let mut w = World::new(single_floor_map());
        w.tunables.god_mode = true;
        w.player.current_life = 5.0;
        w.enemies.push(EnemyInstance {
            id: 1,
            x: w.player.x,
            y: w.player.y,
            archetype: 0,
            combatant: Combatant::dummy(100.0),
            speed: 0.0,
            awake: false,
        });
        w.tick(1.0, &Content::empty());
        assert_eq!(w.player.current_life, 5.0, "god mode should negate contact damage");
        assert!(!w.game_over);
    }

    #[test]
    fn auto_spawn_off_suspends_waves() {
        let mut w = world_at_seed(42);
        w.tunables.auto_spawn = false;
        let content = Content {
            enemies: vec![test_enemy("basic_zombie", 100.0)],
            ..Content::empty()
        };
        w.tick(SPAWN_INTERVAL * 3.0, &content);
        assert!(w.enemies.is_empty(), "no waves while auto_spawn is off");
    }

    #[test]
    fn bullet_damage_tunable_drives_hits() {
        let mut w = World::new(single_floor_map());
        w.tunables.bullet_damage = 50.0;
        w.enemies.push(EnemyInstance {
            id: 1,
            x: w.player.x + 0.3,
            y: w.player.y,
            archetype: 0,
            combatant: Combatant::dummy(1000.0),
            speed: 0.0,
            awake: false,
        });
        w.apply(Command::Fire { dx: 1.0, dy: 0.0 }, 0.016);
        w.tick(0.02, &Content::empty());
        assert!((w.enemies[0].combatant.current_life - 950.0).abs() < 1e-3);
        assert!(matches!(w.last_hit, Some(HitResult::Hit { .. })));
    }

    #[test]
    fn debug_spawn_and_clear() {
        let mut w = world_at_seed(42);
        let content = Content {
            enemies: vec![test_enemy("basic_zombie", 100.0)],
            ..Content::empty()
        };
        w.debug_spawn(0, 5, SPAWN_MIN_DISTANCE, &content);
        assert!(!w.enemies.is_empty(), "debug spawn should place enemies");
        // Unique ids, like wave spawns.
        let mut ids: Vec<u64> = w.enemies.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), w.enemies.len());
        w.debug_clear_enemies();
        assert!(w.enemies.is_empty());
    }

    #[test]
    fn debug_revive_restores_life_after_death() {
        let mut w = World::new(single_floor_map());
        w.player.current_life = 0.0;
        w.tick(0.1, &Content::empty());
        assert!(w.game_over);
        w.debug_revive();
        assert!(!w.game_over);
        assert_eq!(w.player.current_life, w.player.max_life);
    }

    // ---- determinism of the event model ----

    #[test]
    fn same_seed_same_spawn_outcome() {
        let content = Content {
            enemies: vec![
                test_enemy("basic_zombie", 100.0),
                test_enemy("fast_zombie", 60.0),
            ],
            ..Content::empty()
        };
        let mut a = world_at_seed(7);
        let mut b = world_at_seed(7);
        a.tick(SPAWN_INTERVAL, &content);
        b.tick(SPAWN_INTERVAL, &content);
        assert_eq!(a.enemies.len(), b.enemies.len());
        for (ea, eb) in a.enemies.iter().zip(b.enemies.iter()) {
            assert_eq!(ea.archetype, eb.archetype);
            assert!((ea.x - eb.x).abs() < 1e-6);
            assert!((ea.y - eb.y).abs() < 1e-6);
        }
    }
}
