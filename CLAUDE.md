# Project: BoxHead-Inspired ARPG (wasm)

A top-down twin-stick shooter with ARPG progression, procgen levels, and a deep loot system. Inspired by BoxHead 2 for combat feel, Crimsonland/Nation Red for arena flow, and Diablo II/III for loot-rarity flow. The stat-aggregation math is borrowed from Path of Exile / Last Epoch — it's the de-facto ARPG standard and we don't need to reinvent it.

Target: browser via WebAssembly. Single-player first; multiplayer is explicitly out of scope until v1 ships.

---

## Design tone

Lean **Diablo II/III over Path of Exile**. PoE's stat-aggregation formula is the cleanest model in the genre, but the *feel* of the loot system should be Diablo-flavored:

- **No magic, no elemental fantasy** — this is a zombie shooter. Damage types are real ammunition / firearm archetypes (bullet, incendiary, armor-piercing, explosive); defenses are kinetic and biological (life, armor, evasion, thorns, damage-reduction). No fire/cold/lightning resistance, no spell damage, no enchantment language. "Diablo II/III" here means *D2's loot feel*, not its fantasy theming.
- **More rarities, more visible upgrade moments** — 5-tier (Basic → Legendary), not 3. Each tier is structurally distinct: affix count plus a minimum-tier floor, so higher rarities can never roll T4 slop.
- **Drop excitement over crafting depth** — no trade economy, no currency orbs in v1, no scour-then-craft loops.
- **Power fantasy over min-maxing** — affix variety, big numbers, satisfying drops. Build optimization is the player's choice, not a requirement to clear content.
- **Legendary as v1 pinnacle** — standalone uniques (items with named bespoke mods that change gameplay) are deferred. A Legendary today is a roll-floor + affix-count guarantee, not a special template.

When a mechanic could go either way (crafting currency? deterministic re-roll? passive tree of size N?), default to the Diablo answer first. Add PoE-shaped depth only when a specific gameplay problem needs it.

---

## Multiplayer posture

**v1 is single-player.** Coop (cooperative PvE) is on the roadmap for a later release using **[SpacetimeDB](https://spacetimedb.com)** as the netcode + state-sync layer. PvP arenas are a separate, future scope after coop is solid and will likely need additional layers on top.

### Why SpacetimeDB for coop

The shape fits this codebase almost exactly: Rust modules server-side, table-based world state with subscribe-and-react push to clients, transactional reducers as the request shape, ACID rollback on failure. Our `core` crate (`roll_item`, `aggregate_item`, `resolve_hit`, `simulate_fight` — all pure, seeded, deterministic) is exactly what reducers want; we should expect to **reuse `core` unchanged inside the SpacetimeDB module**, not rewrite it.

Reference: `.attic/context/spacetime-db.md` has the docs snapshot.

### Architectural habits to keep

Retrofitting netcode is expensive even with SpacetimeDB. Three habits, adopted now, keep multiplayer cheap to add without slowing single-player iteration:

1. **Inputs are a command stream.** Player input emits `Command` enum variants, not direct state mutation. Trivially serializable, replayable — and on the coop branch, each command type maps to one SpacetimeDB reducer. *Already concrete in v1*: `h2b_game::Command::{Move, Fire}`; the macroquad layer never mutates the world directly, it just emits commands into `World::apply`.
2. **Game state is data, not pointers.** Items already carry their seed and serialize via RON — extend the same instinct to enemies, projectiles, and world chunks. No `Rc<RefCell<>>` graphs, no global singletons. On the coop branch, this state becomes the SpacetimeDB table schema; if it can't serialize, it can't replicate.
3. **`core` stays headless and deterministic.** No `thread_rng`, no `std::time::now()` (use `ctx.timestamp` once we're in reducer-land), no reaching into the renderer from logic. Same-seed reproducibility (already in tests) is the load-bearing property for both client-side prediction and reducer rollback.

### Update granularity (the v1.X design problem)

Can't push 60fps position updates through tables — wire traffic and reducer overhead will both choke. Standard MMO partition once coop work starts:

- **Persistent / replicated state** in tables: player position checkpoints, current_life, inventory, dropped loot, enemy spawn rows, kill events.
- **Ephemeral state** computed client-side from inputs and snapshots: projectile trajectories (replay from a `fired_at` timestamp + initial vector), particles, screen shake, animation tweens.

Player movement is the classic friction point — likely a sparse checkpoint table (10-20Hz) with client-side interpolation, plus occasional input commands as reducers.

### PvP, when it eventually lands

PvP arenas probably need more than subscribe-and-react. Sub-100ms competitive responsiveness wants authoritative-server tick + client-side prediction + rollback. SpacetimeDB stays useful for **matchmaking, persistence, leaderboards, character data**; the in-arena tick loop may live in a thinner layer on top (or fully separate). Defer the decision until PvP is actually being scoped.

PvP balance must be **separate from PvE** (damage caps, CC limits, distinct stat curves) — never try to balance one set of numbers for both modes.

### Before committing fully

One spike worth doing before we lean hard on SpacetimeDB: verify the **Rust client SDK compiles to wasm cleanly** and plays nicely with macroquad's async story. If that's broken, the TypeScript client + Rust core split via wasm interop is plan B. Do this spike when we're close to starting coop work, not now.

---

## Stack

- **Language:** Rust
- **Engine:** `macroquad` for the prototype (lightweight, clean wasm story, fast iteration). Re-evaluate Bevy at the point where ECS-shaped pain shows up — not before.
- **ECS:** `hecs` (small, embeddable, no framework lock-in). Only adopt if hand-rolled component storage starts hurting.
- **Serde:** `serde` + `ron` for content files. RON over JSON/TOML because it matches Rust's type system and round-trips enums cleanly.
- **RNG:** `rand` with explicit `StdRng` seeded from a known source. **Never** use `thread_rng` for anything gameplay-affecting — determinism matters for procgen, loot reproducibility, and debugging.
- **Persistence (web):** IndexedDB via `gloo-storage` or similar. Save on debounce, test the "tab closed mid-fight" path early.
- **Build:** `cargo` targeting `wasm32-unknown-unknown` for web, loaded by macroquad's own JS shim (`mq_js_bundle.js`) — **not** `wasm-bindgen`. macroquad's loader provides the raw wasm imports itself; pulling in wasm-bindgen glue (via gilrs or getrandom's `js` feature) leaves unresolved `__wbindgen_*` imports the loader can't satisfy. `make web` / `make web-serve`; details + caveats in `/web/README.md`. Native build kept working for fast iteration and headless tests.

---

## Repo layout

```
/crates
  /core           ✅ pure logic: stats, affixes, items, combat math, progression, upgrades
  /content        ✅ RON files: affixes, base items, attachments, enemies (+ loaders)
  /sim            ✅ CLI loot simulator (CSV + summary modes, snapshot-locked)
  /procgen        ✅ BSP map generation, flow-field pathing, weighted spawn placement
  /procgen-viz    ✅ CLI visualizer for procgen output (map / flow / spawn picks)
  /game           ✅ boxy-3D macroquad shell: window, WASD movement (wall sliding),
                     mouse-aimed shooting (ground-plane raycast aim), wave-spawned
                     enemies (FlowField pathing), projectile hit detection via
                     core::resolve_hit, loot drops + walk-over pickup, contact damage,
                     floating enemy health bars, death/restart. Angled BoxHead
                     follow-cam. Live-tuning egui
                     debug overlay behind `--features debug` (runtime Tunables).
/assets           ⏳ sprites, audio (placeholder/CC0 until art pipeline exists)
/web              ✅ wasm build: index.html + macroquad JS loader, built via
                     `make web` (keyboard/mouse; no gamepad/debug overlay — see README)
```

Keep `core` free of rendering and IO dependencies. It must compile and run in a headless test or CLI sim without dragging in macroquad. This is the single most important architectural rule — it's what makes the loot simulator possible and what keeps unit tests fast.

---

## Core systems

### Stat aggregation

Three-tier formula. Adopt it as-is; players already understand it from PoE/Last Epoch/Grim Dawn.

```
final = (base + sum_of_flat) × (1 + sum_of_increased) × product_of_more_multipliers
```

- **Flat added:** "+20 bullet damage" — additive with base
- **Increased:** "+15% increased weapon damage" — all sum into one bucket, multiply once
- **More:** "20% more damage while moving" — each applied separately, fully multiplicative

Recompute on equipment change, level up, or buff application/expiry. **Never per frame.** Cache the result on the entity.

### Item model

Three layers:

1. **BaseItem** — what is this (Pistol, Combat Helmet, …). Has intrinsic stats (per-shot damage, fire rate, base life/armor/evasion, …), a `slot` (where it equips), and a `category` used to filter eligible affixes. Weapon categories split by archetype — `rapid_fire` (pistol, SMG) vs `heavy` (shotgun, rocket launcher) — so flat-damage affix ranges can be calibrated per class (a +10 flat bullet roll is 100 DPS on an SMG vs 5 DPS on a rocket — content compensates instead of trying to express it through a single formula).
2. **Affix** — a rollable modifier template. Has tiers (T1 best → T6 worst), each gated by item level with a weight and stat roll ranges. Has a `group` (mutual exclusion within a group on one item) and `tags` (for crafting later).
3. **ItemInstance** — what actually dropped. Stores base, ilvl, rarity, rolled prefixes/suffixes, and **the seed**. Always store the seed — it enables compact saves, debugging, shareable item codes, and replay. Also carries `upgrade_tier` (0..=5, player-applied — see "Upgrades and attachments") and `attached` (list of attachment template IDs slotted on this item).

### Rarities

Five tiers; each differs along two axes — **affix count** and **tier floor** (the best minimum tier an affix is allowed to roll, where T1 is best and T4 is worst). The drop curve below is a starting point — retune with the sim.

| Rarity      | Affix count | Tier floor      | Drop %  |
|-------------|-------------|-----------------|---------|
| Basic       | 0           | —               | 60.0    |
| Common      | 1–2         | none (any tier) | 28.0    |
| Rare        | 3–4         | none (any tier) |  9.0    |
| Epic        | 4–5         | T3 (no T4)      |  2.5    |
| Legendary   | 5–6         | T2 (T2 or T1)   |  0.5    |

The tier floor is what makes Epic and Legendary feel like real upgrades rather than "Rare with one more roll." A Legendary at ilvl 60 cannot roll worse than T2 on any affix slot.

Cap each side at 3 prefixes / 3 suffixes — the affix count above splits across both. If the requested rarity's tier floor can't be filled at this ilvl (e.g. a Legendary at ilvl 20 — T2 needs ilvl 40), `Rarity::downgrade_to_satisfiable` walks down to the highest rarity whose floor *is* achievable. Mirrors Diablo's level-gated drops: endgame rarities just don't appear before the content supports them.

### Roll pipeline

```
1. Drop chance check
2. Rarity roll (weighted, modified by player luck/MF)
3. Base roll (weighted, filtered by ilvl + slot)
4. Affix count (per rarity: Basic 0, Common 1–2, Rare 3–4, Epic 4–5, Legendary 5–6)
5. For each affix slot:
   a. Filter pool: allowed_categories ⊇ base, tier ilvl_required ≤ item ilvl, tier ≤ rarity tier floor, group not taken, prefix/suffix slot open
   b. Weighted pick of affix
   c. Weighted pick of eligible tier within affix
   d. Roll each stat between min/max
6. Compute display name from prefix + base + suffix fragments
```

### Upgrades and attachments

Two player-side layers on top of rolled drops, both Diablo-shaped (no crafting currencies, no scour loops):

- **Upgrade tier** (`ItemInstance.upgrade_tier`, 0..=5). Each drop starts at tier 0; players spend `scrap` (from disenchanting other drops) to bump it. Each tier scales every aggregated stat by +8% — so a fully upgraded item is 1.40× its drop output. Scrap economy in `core::upgrade`: disenchant value is rarity-based (1 / 3 / 10 / 30 / 100), upgrade cost is geometric (5 / 15 / 40 / 100 / 250 — 410 total to max). No refund on disenchant so over-investment has a real cost.
- **Attachments** (`ItemInstance.attached`). Static modifier packs (no rolling for v1) slotted into compatible weapon slots: optic / magazine / barrel / stock. Each `BaseItem.attachment_slots` lists which slots a weapon has. `core::attach::try_attach` is the single validating entry point — checks slot existence, category compatibility, and one-attachment-per-slot, mutating only on success.

Aggregation flows: `core::aggregate_item` folds intrinsics + rolled affixes + attachment modifiers through the three-tier formula, then applies upgrade scaling at the end. Single canonical "what does this item do" entry point.

### Combat

Two layers on the same primitives, sharing the same `Combatant` / `Weapon` types:

- **Expected-value** (`dps_against`, `time_to_kill`) — no RNG. The design/balance lens; what the sim's avg_dps / TTK columns measure.
- **Stochastic** (`resolve_hit`, `simulate_fight`) — RNG threaded through, per-shot dodge and crit rolls. The realtime tick loop's view. A property test (`stochastic_mean_ttk_tracks_expected`) over 200 fights ties the two layers together so they can't drift silently.

Defenses: dodge = `evasion / (evasion + 100)` capped at 50%; armor mitigation = `armor / (armor + 10 × hit_damage)` capped at 85% — diminishing returns, big hits punch through harder than small ones. The armor curve is why rocket beats SMG against the boss while SMG beats rocket against unarmored swarms; no special-cased "AP weapon" content needed for that texture.

### Procgen

Approach decided: **BSP** (binary space partition). Recursive rect-splitting, one room per leaf, L-corridors connecting sibling room centers on the way back up. Deterministic from a `u64` seed; renderer-agnostic.

Companion systems in the same crate:

- **`FlowField`** — 4-connected BFS distance field from a goal (typically the player), computed once per player-tile-change. Enemies steer down it in O(1) per enemy — classic BoxHead-style swarm convergence with no per-enemy pathing work. A connectivity property test ("every Floor on a BSP map is reachable from spawn") doubles as a procgen invariant. Two readers: `next_step_from` (discrete best 4-neighbor) and **`steer_from`** (smooth) — the latter bilinearly samples the distance field and follows its negative gradient for continuous 8-directional motion, sampling walls as a penalty above the local distance so the gradient *bends around* pillars/corners instead of clipping in. At a symmetric saddle (both detours equal, gradient cancels) it returns `None` and the caller falls back to `next_step_dir` (the discrete tiebreak) to pick a side. `h2b_game::move_enemies` uses `steer_from` with per-axis wall-slide; the debug flow viz draws the same `steer_from` directions enemies actually follow.

  **Aggro (game side).** Enemies spawn **dormant** (`EnemyInstance.awake = false`) and hold position. A per-tick awareness pass wakes any dormant enemy that spots the player — within `sight_range` *and* clear line of sight — and getting shot also wakes one. The latch is permanent: once awake an enemy gives chase via the steering below even after losing sight. Only awake enemies move. (`debug_wake_all` + the panel's "wake all" button aggro everything for testing; the floating stat block tags dormant enemies `[idle]`.)

  **Steering blend (game side).** A single-goal flow field alone funnels every enemy onto the player's tile (and 4-connected BFS = Manhattan distance, whose gradient approaches at 45° then snaps to an axis — enemies pile up directly in front). `move_enemies` fixes the *feel* with two layers on top of the field: (1) a **line-of-sight beeline** — when the player is within `los_range` *and* the straight enemy→player segment is wall-free, the enemy heads straight at the player (radial from any angle); beyond the range or behind a wall it falls back to `steer_from` (the range check also short-circuits the LOS sampling for distant enemies); (2) **boids separation** — enemies push apart within `separation_radius`, so a swarm rings the player instead of stacking on one point. All live tunables (`los_range`, `separation_weight`, `separation_radius`); separation runs as a two-pass snapshot (compute all directions, then apply) to stay order-independent and deterministic.
- **`pick_spawn_points`** — Floor tiles weighted linearly by distance from goal, filtered by `min_distance`, no duplicates. Avoids "swarm spawns on top of the player" without special casing.

Hand-authored room templates and biomes are deferred — BSP rooms are enough scaffolding for v1, templates layer on top later when content scope demands them.

---

## Conventions

- **Pool everything in hot paths.** No per-frame allocation for projectiles, particles, damage numbers. Pre-size pools.
- **Batch the JS↔wasm boundary.** Expose `tick(dt)` and `get_render_buffer()`, not per-entity calls.
- **Sprite batching:** one draw call per texture atlas per frame. Pack aggressively.
- **Content is data, not code.** Affixes, base items, enemies, drop tables → RON files in `/crates/content`. Hot-reload in dev builds.
- **Determinism:** seeded RNG threaded through all generation. Every drop, every map, every encounter should be reproducible from `(world_seed, event_id)`.
- **Tests:** `core` has unit tests for stat math, roll mechanics, combat, aggregation, attachments, progression. `procgen` has BFS / connectivity / spawn-bias property tests. `sim` has snapshot tests (`insta`) that lock CSV + summary output for fixed seeds. Run native, all fast.
- **Error handling:** `Result` + `thiserror` for library errors. `anyhow` only at binary boundaries.

---

## The loot simulator (built — keep using it)

`crates/sim` is the project's primary tuning tool. Built first, before rendering / procgen / combat had any callers, exactly as the original plan called for. **Run it whenever content or roll math changes.**

```
cargo run -p head2box-sim -- --monster-level 60 --kills 20000 --seed 42 --summary
```

Two modes:

- **CSV** (default): one row per drop with columns `kill, rarity, base, ilvl, n_affixes, dps_estimate, ttk_estimate, affixes`. Pipe into `duckdb` / `pandas` for ad-hoc slicing.
- **`--summary`**: distribution tables in one screen — rarity counts, per-base avg_dps / avg_ttk / kit_dps / kit_ttk (with optimal attachments slotted), per-rarity TTK breakdown, per-enemy TTK matrix, kills-to-reach-each-level table, and per-affix tier histograms with avg_roll vs theoretical range.

Eyeballing the summary answers: are T1 affixes appearing at the right rate? Does the average Rare feel meaningfully better than a Common? Is each weapon archetype best against something? How grindy is each enemy? Snapshot tests lock the output for `seed=42, ilvl=20` and `seed=42, ilvl=60` so accidental regressions in roll / aggregate / combat math fail fast.

---

## The in-game debug overlay (feel-tuning, the sim's realtime counterpart)

Where the sim tunes *numbers offline*, the debug overlay tunes *feel live* — the
egui panel for the questions you can only answer by playing: does enemy speed ×
feel oppressive, is the fire rate satisfying, how dense should waves be.

```
cargo run -p head2box-game --features debug   # or: cargo dbg
```

Feature-gated: the `debug` feature pulls the local-dev-only deps (`egui-macroquad`,
`serde`/`ron` for tunable export, and `clap` for CLI parsing), so a normal
`cargo build`/`--release` and the wasm build compile none of it. **F1** toggles
the panel. `.cargo/config.toml` defines shortcut aliases — `cargo dbg` (BSP
dungeon), `cargo arena`, `cargo arena-empty`. Aliases can't set env vars, so the
level rides in as a CLI arg (`-- arena`); under the `debug` feature a clap parser
(`Cli`) reads it (plus `--seed`, `--help`), and a non-debug build falls back to
reading `H2B_LEVEL` / `H2B_SEED` env vars with no arg parser linked in.

Mechanism — the load-bearing refactor: every gameplay knob that used to be a
`const` now lives in `h2b_game::Tunables` on `World`, and the simulation reads
**only** from there. The `const`s remain the default source (`Tunables::default`
snapshots them), so behaviour and tests are unchanged until something mutates a
field. The panel binds sliders straight to `world.tunables` (damage, fire rate,
projectile speed, contact dps, player/enemy speed, spawn cadence + caps, drop
chance), plus god mode and an auto-spawn toggle. Manual spawning, clear/revive,
and a per-shot hit readout (damage dealt / crit / dodge, surfaced from the
otherwise-discarded `resolve_hit` result) go through `World::debug_*` methods.
Export/import round-trips the whole `Tunables` block to `head2box-tunables.ron`
(pretty RON, hand-editable) in the working dir — dial in a feel, export it, and
the file is a reusable preset (serde derives are debug-gated, so non-debug
builds stay serde-free).

Those `Tunables` / `debug_*` surfaces live on the headless `World` (not behind
the feature) — only the egui *editor* is gated. Keeps the lib identical for
tests and the eventual SpacetimeDB reuse; difficulty presets later are just
another `Tunables` producer.

### The debug level (arena) — weapons, TTK, pathfinding

The overlay doubles as a test harness. Launch straight into an open arena with
`H2B_LEVEL=arena` (or `arena-empty` for no pillars; anything else → the BSP
dungeon), or hot-swap maps from the overlay's **level / map** section. The arena
(`h2b_procgen::generate_arena`) is a big bordered room with an optional 2×2
pillar grid — open sightlines for tuning, real geometry for watching pathing.
The same fully-connected invariant the BSP maps hold is property-tested. Both
arena entry paths (launch and overlay button) start with **auto-spawn off** so
you populate it deliberately rather than getting swarmed on entry.

```
cargo arena                                    # shortcut alias
H2B_LEVEL=arena cargo run -p head2box-game --features debug   # equivalent
```

Three tuning levers beyond the raw sliders:

- **Weapon picker.** Select any weapon base and "equip" loads its real stats
  (damage / fire rate / crit) into the tunables via the canonical
  `aggregate_item → Weapon::from_stats` path — the *same* numbers the sim
  reports, so realtime feel and offline balance stay tied. Crit is now a tunable
  threaded onto the projectile (`Tunables::crit_chance/_multiplier`), defaulting
  to no-crit so shipping behaviour is unchanged.
- **Live TTK readout.** Expected DPS and time-to-kill of the current tunables
  weapon against the selected enemy archetype (`dps_against` / `time_to_kill`),
  updating as you drag sliders — the realtime mirror of the sim's TTK matrix.
- **Flow-field viz.** "show flow field" draws the actual `FlowField` next-step
  arrows enemies steer by (cyan arrows downhill toward the player, yellow goal
  pad), so you can see a swarm route around pillars and through gaps before
  committing pathing changes. Reads `World::flow()` (a debug-only accessor).
- **Entity stat blocks.** "show entity stats" floats an `Enemy`/`Combatant`
  readout above each nearby enemy (id, ilvl, hp, armor/evasion, speed, distance
  + flow distance) and a `PLAYER` block (hp, pos, fire cooldown, live counts).
  3D positions are projected to the 2D HUD via the camera matrix
  (`world_to_screen`) — all read-only field access, no lib changes.

---

## Initial scope (v1 — playable demo)

Deliberately tight. Expand only after this is fun. Status markers track current state — ✅ done, ⏳ remaining.

- ✅ **Weapons:** 4 archetypes in 2 categories — `rapid_fire` (pistol, SMG) and `heavy` (shotgun, rocket launcher). Category drives damage-affix calibration; see BaseItem under Core systems.
- ✅ **Armor:** 3 slots — helm, chest, boots
- ✅ **Affixes:** 20 affixes × 4 tiers (T5/T6 skipped per scope)
- ✅ **Attachments:** 9 templates across 4 slot types (optic / magazine / barrel / stock)
- ✅ **Rarities:** 5-tier (Basic / Common / Rare / Epic / Legendary) with tier-floor and ilvl-gated downgrade. Standalone uniques deferred.
- ✅ **Enemies:** 5 types — basic zombie, fast zombie, fat/tank, ranged spitter, swarm rusher
- ✅ **Bosses:** 1 (Patient Zero)
- ⏳ **Biomes:** 1 — deferred; BSP procgen produces a single aesthetic for now, biome variation layers on later.
- ⏳ **Room templates:** 20 — deferred; v1 uses generic BSP rooms. Hand-authored templates can stitch in when content scope demands it.
- ✅ **Progression:** XP curve to level 30. Passive tree skipped per the "or skip" branch of the original spec.
- ✅ **Game runtime (window + movement + shooting):** boxy-3D macroquad shell with WASD/arrow movement (per-axis wall sliding) and mouse-aimed shooting (cursor raycast onto the ground plane). Projectiles fly with cooldown, despawn on wall collision or lifetime expiry. The `Command` stream pattern is concrete here. Also takes **twin-stick gamepad** input via `gilrs` (left stick = move, right stick = aim, right trigger = fire); the `pad` module resolves one `PadInput`/frame and `collect_input` merges it with keyboard/mouse (either works). **Native only:** gilrs's web backend reaches the browser Gamepad API through `wasm-bindgen`, whose glue macroquad's plain loader can't satisfy, so gilrs is a native-only dependency and the web build ships a no-op `Pads` stub (`#[cfg(target_arch = "wasm32")]`). Gamepad-on-web is deferred; keyboard/mouse cover the web build.
- ✅ **Game runtime (enemies + hit detection + loot pickup):** waves spawn via `pick_spawn_points`, path to the player with `FlowField` (recomputed per player-tile-change), take projectile hits through `core::combat::resolve_hit` (enemy armor/dodge applies), and on death award XP + roll an `ItemInstance` drop the player walks over to collect. Touching enemies drain life; zero life → death + restart. Loot rolls and spawns are seeded per-event from `(world_seed, event_id)`; every enemy/drop carries a stable id (the eventual table key + interpolation match key). No player loadout yet — bullet damage/fire rate are still constants, not gear-derived.
- ⏳ **Persistence:** IndexedDB save on debounce (web target). The "tab closed mid-fight" path needs to work before save format gets locked.

---

## Out of scope (be ruthless)

- Multiplayer / netcode (coop on the roadmap via SpacetimeDB, PvP after — see Multiplayer posture)
- Trade
- Seasons / leagues
- Uniques
- Crafting currencies
- Skill tree until core combat feels good
- Custom art (use CC0 placeholders; art pipeline is a separate project)
- Mobile / touch controls

---

## Personal context for Claude

- Developer is comfortable in Rust-adjacent territory but daily-drives Go. Don't over-explain idiomatic Rust unless asked; do flag non-obvious lifetime/borrow patterns.
- Direct, concise communication preferred. Skip preamble.
- Czech or English both fine.
- Background includes YAML/Viper-style data-driven config — RON content authoring should feel natural.
- Prior project (Frag Factor, Go + SQLite + Viper) used similar separation between engine and content; apply the same instinct here.

---

## Open questions to resolve early

1. macroquad vs Bevy — start with macroquad, but set a tripwire: if we end up reinventing systems/queries, switch. **Still open** — game runtime not yet started.
2. Save format versioning strategy — bump-on-break with migrations, or backwards-compatible? Decide before the first persistent save.
3. ~~Procgen approach — BSP, WFC, chunk-stitched, or hybrid?~~ **Resolved: BSP**. Recursive space-partition with L-corridor connections, deterministic from seed. Hand-authored room templates can layer on top later if needed; the BSP cleanly partitions the work.
4. Audio stack — `kira` is the current best-in-class for Rust games, works in wasm. Confirm at the point audio matters.