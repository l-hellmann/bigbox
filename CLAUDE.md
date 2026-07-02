# Project: bigbox — BoxHead-Inspired ARPG (Bevy)

A top-down twin-stick shooter with deep ARPG progression, procgen levels, and a deep loot system. Inspired by BoxHead 2 for combat feel, Crimsonland/Nation Red for arena flow, and Diablo II/III for loot-rarity flow. The stat-aggregation math is borrowed from Path of Exile / Last Epoch — it's the de-facto ARPG standard and we don't need to reinvent it.

Target: **desktop-first** — native builds for Windows / macOS / Linux. Single-player first; **coop / PvP multiplayer is a committed roadmap item** for this product (see Multiplayer posture). **Web/wasm is explicitly NOT a target** — dropping it removes Bevy's one real downside (large wasm binaries) and unlocks the full native rendering backend.

> **Fork note (2026-07-02).** `bigbox` is the **deep-ARPG** half of a two-way split from `head2box`. `head2box` stays a lean **macroquad twin-stick arena shooter** (PvE + local/online arena MP); `bigbox` keeps the full loot system and moves to **Bevy** because the ceiling we hit was *rendering/assets* — macroquad has no GLTF/skeletal-animation/PBR/lighting, and building that on raw miniquad means writing a 3D engine.
>
> The two repos are **duplicate-and-diverge**, not a shared workspace: shared logic (`core`, `content`, `procgen`, the engine-agnostic `game` World/sim) was copied in and now evolves independently here. Genuine bugfixes to shared logic get hand-ported between repos.
>
> **Migration status (2026-07-02): COMPLETE.** The `game` crate now runs entirely on **Bevy 0.19** — window/camera, input (`bevy_input` + `bevy_gilrs`), 3D scene (`scene.rs`), HUD + inventory (`bevy_ui`, `hud.rs`), and the debug tuning overlay (`bevy_egui`, `debug.rs`, `--features debug`). **macroquad is fully removed** from the workspace. The headless simulation (`game/lib.rs` — World, tick, `Command` stream) ported unchanged, as planned. Crates were renamed `bigbox-*` / `bb_*` in Phase 0. The next epic is the *payoff* the move unlocked: GLTF/PBR/lighting/skeletal-animation/hot-reload (see "Deferred epic" at the end). Full phase log in `.attic/migration/plan.md`.

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

**v1 ships single-player**, but multiplayer is a *committed* direction for this product (not a "maybe someday"): coop (cooperative PvE) first, PvP after. The netcode + state-sync layer is **[SpacetimeDB](https://spacetimedb.com)**. PvP is a separate, later scope after coop is solid and will likely need additional layers on top. Keep the architectural habits below from day one so the port stays cheap — this matters *more* here than it did pre-split, because multiplayer is on this repo's actual roadmap.

> **Bevy interaction:** the SpacetimeDB module is engine-agnostic Rust and reuses `core` unchanged regardless of the client. On the Bevy client side, subscribe-and-react table updates map naturally onto Bevy systems/events, and the **native** Rust client SDK sidesteps wasm-client friction entirely — desktop-only makes the netcode integration meaningfully simpler.

### Why SpacetimeDB for coop

The shape fits this codebase almost exactly: Rust modules server-side, table-based world state with subscribe-and-react push to clients, transactional reducers as the request shape, ACID rollback on failure. Our `core` crate (`roll_item`, `aggregate_item`, `resolve_hit`, `simulate_fight` — all pure, seeded, deterministic) is exactly what reducers want; we should expect to **reuse `core` unchanged inside the SpacetimeDB module**, not rewrite it.

Reference: `.attic/context/spacetime-db.md` has the docs snapshot.

### Architectural habits to keep

Retrofitting netcode is expensive even with SpacetimeDB. Three habits, adopted now, keep multiplayer cheap to add without slowing single-player iteration:

1. **Inputs are a command stream.** Player input emits `Command` enum variants, not direct state mutation. Trivially serializable, replayable — and on the coop branch, each command type maps to one SpacetimeDB reducer. *Already concrete*: `bb_game::Command::{Move, Fire}`; the render/input layer never mutates the world directly, it just emits commands into `World::apply`. (This stays true across the Bevy port — the Bevy input systems emit the same `Command`s.)
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

One spike worth doing before we lean hard on SpacetimeDB: verify the **native Rust client SDK integrates cleanly with Bevy's runtime** (the SDK's own async/threading vs Bevy's schedule). Desktop-only removes the old wasm-compilation risk entirely — this is now a straightforward native-integration check. Do this spike when we're close to starting coop work, not now.

---

## Stack

- **Language:** Rust
- **Engine:** **Bevy** (chosen over macroquad — resolves open question #1). The pain that triggered the split was *rendering/assets*: we need GLTF import with skeletal animation, PBR materials, lighting/shadows, a real transform hierarchy, and an asset pipeline with hot-reload — all first-class in Bevy, none present in macroquad (which is primitives + hand-written GLSL only). Bevy's ECS also retires the "reinventing systems/queries" tripwire permanently. Desktop-native is Bevy's strongest, best-supported target — the wasm binary-size problem doesn't apply. Accepted costs: Bevy's per-release API churn and slower cold builds (mitigate with dynamic linking in dev).
- **ECS:** **Bevy's built-in ECS.** (The old `hecs` note is moot — Bevy *is* the ECS.) The engine-agnostic `game` simulation can stay a monolithic `World::tick` called from one Bevy system to de-risk the port, then decompose into ECS systems where it pays off. Don't decompose everything up front.
- **Serde:** `serde` + `ron` for content files. RON over JSON/TOML because it matches Rust's type system and round-trips enums cleanly. (Bevy's asset layer can load RON via `bevy_common_assets` or a custom `AssetLoader` for in-engine hot-reload; the headless `content` loaders stay as-is for sim/tests.)
- **RNG:** `rand` with explicit `StdRng` seeded from a known source. **Never** use `thread_rng` for anything gameplay-affecting — determinism matters for procgen, loot reproducibility, and (now doubly) client-side prediction + reducer rollback.
- **Persistence:** native save files — a per-user save dir (via `directories`/`dirs`), RON or bincode. Save on debounce; test the "quit/crash mid-fight" recovery path early.
- **Rendering backend:** native `wgpu` — Vulkan (Linux/Windows), Metal (macOS), DX12 (Windows). No WebGL2/web feature ceiling, so shadows, compute, and modern material features are all on the table.
- **Build:** plain `cargo build` / `cargo run` per platform. **No wasm, no wasm-bindgen, no JS glue** — head2box's entire `/web` chain (`mq_js_bundle.js`, `quad-url`, `quad-gamepad.js`, the custom-getrandom stub) is *deleted*, not ported. Native gamepads via `bevy_gilrs` (or `gilrs` directly — no web-backend/wasm-bindgen friction anymore). Distribution is a native binary + assets folder per OS.

---

## Repo layout

```
/crates
  /core           ✅ pure logic: stats, affixes, items, combat math, progression, upgrades
  /content        ✅ RON files: affixes, base items, attachments, enemies (+ loaders)
  /sim            ✅ CLI loot simulator (CSV + summary modes, snapshot-locked)
  /procgen        ✅ BSP map generation, flow-field pathing, weighted spawn placement
  /procgen-viz    ✅ CLI visualizer for procgen output (map / flow / spawn picks)
  /game           ✅ game logic + Bevy shell. `lib.rs` (World, tick, Command
                     stream, movement, enemies, projectiles, waves, loot, rack +
                     inventory) is engine-agnostic. The shell is Bevy systems:
                     `main.rs` (App, Camera3d follow-cam, resources), `scene.rs`
                     (boxy-3D meshes/materials/gizmos), `input.rs`+`pad.rs`
                     (bevy_input + bevy_gilrs → Command stream), `hud.rs`
                     (bevy_ui HUD + inventory), `debug.rs` (bevy_egui overlay,
                     `--features debug`).
/assets           ⏳ GLTF/GLB models + animations, textures, audio (CC0
                     placeholders until art pipeline exists) — loadable
                     through Bevy's asset server with hot-reload.
(no /web)          — desktop-only; head2box's /web wasm chain is not carried over.
```

Keep `core` free of rendering and IO dependencies. It must compile and run in a headless test or CLI sim without dragging in Bevy. This is the single most important architectural rule — it's what makes the loot simulator possible, what keeps unit tests fast, and what lets `core` drop unchanged into a SpacetimeDB reducer.

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

- **`FlowField`** — 4-connected BFS distance field from a goal (typically the player), computed once per player-tile-change. Enemies steer down it in O(1) per enemy — classic BoxHead-style swarm convergence with no per-enemy pathing work. A connectivity property test ("every Floor on a BSP map is reachable from spawn") doubles as a procgen invariant. Two readers: `next_step_from` (discrete best 4-neighbor) and **`steer_from`** (smooth) — the latter bilinearly samples the distance field and follows its negative gradient for continuous 8-directional motion, sampling walls as a penalty above the local distance so the gradient *bends around* pillars/corners instead of clipping in. At a symmetric saddle (both detours equal, gradient cancels) it returns `None` and the caller falls back to `next_step_dir` (the discrete tiebreak) to pick a side. `bb_game::move_enemies` uses `steer_from` with per-axis wall-slide; the debug flow viz draws the same `steer_from` directions enemies actually follow.

  **Aggro (game side).** Enemies spawn **dormant** (`EnemyInstance.awake = false`) and hold position. A per-tick awareness pass wakes any dormant enemy that spots the player — within `sight_range` *and* clear line of sight — and getting shot also wakes one. The latch is permanent: once awake an enemy gives chase via the steering below even after losing sight. Only awake enemies move. (`debug_wake_all` + the panel's "wake all" button aggro everything for testing; the floating stat block tags dormant enemies `[idle]`.)

  **Steering blend (game side).** A single-goal flow field alone funnels every enemy onto the player's tile (and 4-connected BFS = Manhattan distance, whose gradient approaches at 45° then snaps to an axis — enemies pile up directly in front). `move_enemies` fixes the *feel* with two layers on top of the field: (1) a **line-of-sight beeline** — when the player is within `los_range` *and* the straight enemy→player segment is wall-free, the enemy heads straight at the player (radial from any angle); beyond the range or behind a wall it falls back to `steer_from` (the range check also short-circuits the LOS sampling for distant enemies); (2) **boids separation** — enemies push apart within `separation_radius`, so a swarm rings the player instead of stacking on one point. All live tunables (`los_range`, `separation_weight`, `separation_radius`); separation runs as a two-pass snapshot (compute all directions, then apply) to stay order-independent and deterministic.
- **`pick_spawn_points`** — Floor tiles weighted linearly by distance from goal, filtered by `min_distance`, no duplicates. Avoids "swarm spawns on top of the player" without special casing.

Hand-authored room templates and biomes are deferred — BSP rooms are enough scaffolding for v1, templates layer on top later when content scope demands them.

---

## Conventions

- **Pool everything in hot paths.** No per-frame allocation for projectiles, particles, damage numbers. Under Bevy this means reusing entities/components (or a pool resource) rather than spawn/despawn churn in the swarm-heavy tick.
- **Let Bevy own rendering.** The Bevy ECS *is* the render source of truth — no hand-rolled render buffers. Keep the sim/render split clean: `game/lib.rs` mutates the World; Bevy systems read it and drive `Transform`/mesh/material. Ephemeral visuals (particles, screen shake, tweens) live render-side, never in the replicated World state.
- **Batch by material/mesh.** Bevy auto-batches by pipeline; keep material and mesh handles shared (don't mint a unique material per entity) so instancing kicks in. Pack textures into atlases where it helps.
- **Content is data, not code.** Affixes, base items, enemies, drop tables → RON files in `/crates/content`. Hot-reload in dev builds (headless loaders for sim/tests; Bevy `AssetLoader` for in-engine reload).
- **Determinism:** seeded RNG threaded through all generation. Every drop, every map, every encounter should be reproducible from `(world_seed, event_id)`.
- **Tests:** `core` has unit tests for stat math, roll mechanics, combat, aggregation, attachments, progression. `procgen` has BFS / connectivity / spawn-bias property tests. `sim` has snapshot tests (`insta`) that lock CSV + summary output for fixed seeds. Run native, all fast.
- **Error handling:** `Result` + `thiserror` for library errors. `anyhow` only at binary boundaries.

---

## The loot simulator (built — keep using it)

`crates/sim` is the project's primary tuning tool. Built first, before rendering / procgen / combat had any callers, exactly as the original plan called for. **Run it whenever content or roll math changes.**

```
cargo run -p bigbox-sim -- --monster-level 60 --kills 20000 --seed 42 --summary
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
cargo run -p bigbox-game --features debug   # or: cargo dbg
```

> **Host note:** the overlay runs on **`bevy_egui`** (`debug.rs` is a `bevy_egui` system in the `EguiPrimaryContextPass` schedule). The load-bearing design below (Tunables on `World`, `debug_*` methods, live sliders bound to `world.tunables`) is engine-agnostic and was unchanged by the host swap from `egui-macroquad`.

Feature-gated: the `debug` feature pulls the local-dev-only deps (the egui host,
`serde`/`ron` for tunable export, and `clap` for CLI parsing), so a normal
`cargo build`/`--release` compiles none of it. **F1** toggles
the panel. `.cargo/config.toml` defines shortcut aliases — `cargo dbg` (BSP
dungeon), `cargo arena`, `cargo arena-empty`. Aliases can't set env vars, so the
level rides in as a CLI arg (`-- arena`); under the `debug` feature a clap parser
(`Cli`) reads it (plus `--seed`, `--help`), and a non-debug build falls back to
reading `BB_LEVEL` / `BB_SEED` env vars with no arg parser linked in.

Mechanism — the load-bearing refactor: every gameplay knob that used to be a
`const` now lives in `bb_game::Tunables` on `World`, and the simulation reads
**only** from there. The `const`s remain the default source (`Tunables::default`
snapshots them), so behaviour and tests are unchanged until something mutates a
field. The panel binds sliders straight to `world.tunables` (damage, fire rate,
projectile speed, contact dps, player/enemy speed, spawn cadence + caps, drop
chance, plus the equipped archetype's fire-pattern knobs — shotgun pellet
count + spread half-angle, or rocket blast radius + speed factor — shown only
for the held weapon's profile), plus god mode and an auto-spawn toggle. A **loot** section rolls a drop on demand — pick a base (or random), rarity, and ilvl, dropped on the player (instant pickup, to fill the rack/bag) or at the cursor (`World::debug_drop`, which forces a base by rolling against a one-item pool). Manual spawning, clear/revive,
and a per-shot hit readout (damage dealt / crit / dodge, surfaced from the
otherwise-discarded `resolve_hit` result) go through `World::debug_*` methods.
Export/import round-trips the whole `Tunables` block to `bigbox-tunables.ron`
(pretty RON, hand-editable) in the working dir — dial in a feel, export it, and
the file is a reusable preset (serde derives are debug-gated, so non-debug
builds stay serde-free).

Those `Tunables` / `debug_*` surfaces live on the headless `World` (not behind
the feature) — only the egui *editor* is gated. Keeps the lib identical for
tests and the eventual SpacetimeDB reuse; difficulty presets later are just
another `Tunables` producer.

### The debug level (arena) — weapons, TTK, pathfinding

The overlay doubles as a test harness. Launch straight into an open arena with
`BB_LEVEL=arena` (or `arena-empty` for no pillars; anything else → the BSP
dungeon), or hot-swap maps from the overlay's **level / map** section. The arena
(`bb_procgen::generate_arena`) is a big bordered room with an optional 2×2
pillar grid — open sightlines for tuning, real geometry for watching pathing.
The same fully-connected invariant the BSP maps hold is property-tested. Both
arena entry paths (launch and overlay button) start with **auto-spawn off** so
you populate it deliberately rather than getting swarmed on entry.

```
cargo arena                                    # shortcut alias
BB_LEVEL=arena cargo run -p bigbox-game --features debug   # equivalent
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
- ✅ **Game runtime (window + movement + shooting):** boxy-3D **Bevy** shell — a `Camera3d` angled follow-cam over extruded-cube walls and stacked-cube box-people. WASD/arrow movement (per-axis wall sliding) and mouse-aimed shooting (cursor → ground-plane via `Camera::viewport_to_world`). Projectiles fly with cooldown, despawn on wall collision or lifetime expiry. The `Command` stream pattern is concrete here (`player_input` system → `World::apply`). Also takes **twin-stick gamepad** input via **`bevy_gilrs`** (left stick = move, right stick = aim, right trigger = fire, bumpers = cycle); `pad::read_pad` resolves one `PadInput`/frame from the `Gamepad` component and `input::collect_commands` merges it with keyboard/mouse. Desktop-native only — no wasm gamepad path (deleted with the rest of the web chain).
- ✅ **Game runtime (enemies + hit detection + loot pickup):** waves spawn via `pick_spawn_points`, path to the player with `FlowField` (recomputed per player-tile-change), take projectile hits through `core::combat::resolve_hit` (enemy armor/dodge applies), and on death award XP + roll an `ItemInstance` drop the player walks over to collect. Touching enemies drain life; zero life → death + restart. Loot rolls and spawns are seeded per-event from `(world_seed, event_id)`; every enemy/drop carries a stable id (the eventual table key + interpolation match key). **Player loadout is wired:** the player is armed with a pistol at setup (`World::equip_base`) and a picked-up weapon auto-equips when it's a strict DPS upgrade (`World::equip` → `aggregate_item` → `Weapon::from_stats`, routing damage/fire-rate/crit into `Tunables`, the fire path's read surface). Armor isn't equipped yet — only weapons. **Weapons stack into a switchable rack** (`World::loadout`, a `Vec<EquippedWeapon>` each aggregated once on acquisition) holding **one weapon per archetype** — the best instance of each base. A pickup of a new archetype claims a slot; a better instance of an archetype already racked replaces it (the displaced one falls to inventory); a worse/equal duplicate goes straight to inventory (`World::acquire_item`). `Command::{SwitchWeapon, CycleWeapon}` (number keys / Q-E / mouse wheel / controller bumpers) re-point `active` and reseed the tunables. Switching needs no content (the rack is precomputed), so it resolves in the `apply` command stream with no extra plumbing. **Inventory screen** (`I` / `Tab`, pauses the sim via the `Paused` gate): a **`bevy_ui`** modal (`game/src/hud.rs`) — HUD + inventory render in Quantico (bundled at `game/assets/fonts/quantico` under the OFL, loaded as a Bevy `Font` from the embedded ttf so the binary stays self-contained). Lists the equipped rack (click a slot → `World::switch_weapon`) and the collected-item bag (click a weapon → `World::equip_from_inventory`, which unlike a pickup places the chosen weapon into its archetype slot *unconditionally* and drops the displaced one back to the bag), via Bevy `Interaction`. Armor is listed but not yet equippable. **Archetypes fire distinctly** (game-layer only, keyed by base id in `fire_profile`): pistol/SMG fire a single shot, the shotgun fires a 6-pellet damage-split cone (point-blank ≈ full damage, fans out at range), and the rocket launcher lobs a slow `aoe_radius` shot that detonates on enemy *or* wall impact, dealing full damage to all enemies in the blast (no falloff) plus a transient `Explosion` render marker. The loot sim still models a weapon as one hit, so pellet/AoE can shift real DPS-vs-armor away from the sim's numbers — retune there if it drifts.
- ⏳ **Persistence:** native save-file on debounce. The "quit/crash mid-fight" path needs to work before save format gets locked.

---

## Out of scope (be ruthless)

- Multiplayer / netcode (coop on the roadmap via SpacetimeDB, PvP after — see Multiplayer posture)
- Trade
- Seasons / leagues
- Uniques
- Crafting currencies
- Skill tree until core combat feels good
- Custom art (use CC0 placeholders; art pipeline is a separate project)
- **Web / wasm build** — desktop-first; wasm was dropped as a requirement (revisit only on real demand)
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

1. ~~macroquad vs Bevy~~ **Resolved: Bevy.** Split from head2box; the rendering/assets ceiling (GLTF/skeletal animation/PBR/lighting) forced it. head2box keeps macroquad for the lean wasm arena shooter; bigbox is desktop-native Bevy.
2. Save format versioning strategy — bump-on-break with migrations, or backwards-compatible? Decide before the first persistent save.
3. ~~Procgen approach — BSP, WFC, chunk-stitched, or hybrid?~~ **Resolved: BSP**. Recursive space-partition with L-corridor connections, deterministic from seed. Hand-authored room templates can layer on top later if needed; the BSP cleanly partitions the work.
4. Audio stack — `bevy_audio` for simple needs, or `bevy_kira_audio` (`kira`) for mixing/effects/spatial. Desktop-only drops the browser-format caveats. Confirm at the point audio matters.

---

## Deferred epic: rendering fidelity (the payoff the Bevy move unlocks)

The macroquad → Bevy migration (phases 0–5, complete 2026-07-02) deliberately **replicated the old boxy-primitive look** — same flat colors, stacked cubes, no lighting — so any regression was obviously a port bug, not new art. That's parity, not the point. The reason for the move was the *rendering/assets ceiling* macroquad couldn't clear; cashing that in is the next epic, a separate body of work:

- **GLTF/GLB import with skeletal animation** — real character models + animation clips (replace the stacked-cube box-people), via `bevy_gltf` + the animation graph.
- **PBR materials + lighting + shadows** — currently every material is `unlit: true` with `Tonemapping::None` to match macroquad's flat look. Turn on real lights, PBR, shadow maps; retune colors for the lit pipeline.
- **Asset pipeline + hot-reload** — load models/textures/audio through Bevy's `AssetServer` (dev hot-reload), instead of the `include_bytes!`/`include_str!` embeds inherited for a self-contained binary.
- **Cube wireframe edges** — the one deliberate parity gap: macroquad's `draw_cube_wires` black outlines weren't reproduced (unlit flat cubes). Moot once real models/lighting land; revisit only if the boxy look is kept.

Do this **after** the game is confirmed fun on the parity build, and treat it as its own project (art direction, asset sourcing) — not migration cleanup. The **SpacetimeDB coop spike** (see Multiplayer posture) is an independent parallel track.