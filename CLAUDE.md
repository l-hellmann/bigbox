# Project: BoxHead-Inspired ARPG (wasm)

A top-down twin-stick shooter with ARPG progression, procgen levels, and a deep loot system. Inspired by BoxHead 2 for combat feel, Crimsonland/Nation Red for arena flow, and Path of Exile / Last Epoch for item systems.

Target: browser via WebAssembly. Single-player first; multiplayer is explicitly out of scope until v1 ships.

---

## Stack

- **Language:** Rust
- **Engine:** `macroquad` for the prototype (lightweight, clean wasm story, fast iteration). Re-evaluate Bevy at the point where ECS-shaped pain shows up — not before.
- **ECS:** `hecs` (small, embeddable, no framework lock-in). Only adopt if hand-rolled component storage starts hurting.
- **Serde:** `serde` + `ron` for content files. RON over JSON/TOML because it matches Rust's type system and round-trips enums cleanly.
- **RNG:** `rand` with explicit `StdRng` seeded from a known source. **Never** use `thread_rng` for anything gameplay-affecting — determinism matters for procgen, loot reproducibility, and debugging.
- **Persistence (web):** IndexedDB via `gloo-storage` or similar. Save on debounce, test the "tab closed mid-fight" path early.
- **Build:** `cargo` + `wasm-bindgen` for the wasm target. Native build kept working for fast iteration and headless tests.

---

## Repo layout (proposed)

```
/crates
  /game           # the actual game (rendering, input, game loop)
  /core           # pure logic: stats, affixes, items, combat math — no rendering, no IO
  /content        # RON files: affixes, base items, enemies, encounter tables
  /sim            # CLI loot simulator (see "Build this first")
  /procgen        # level generation
/assets           # sprites, audio (placeholder/CC0 until art pipeline exists)
/web              # wasm bundling, index.html, JS shim
```

Keep `core` free of rendering and IO dependencies. It must compile and run in a headless test or CLI sim without dragging in macroquad. This is the single most important architectural rule — it's what makes the loot simulator possible and what keeps unit tests fast.

---

## Core systems

### Stat aggregation

Three-tier formula. Adopt it as-is; players already understand it from PoE/Last Epoch/Grim Dawn.

```
final = (base + sum_of_flat) × (1 + sum_of_increased) × product_of_more_multipliers
```

- **Flat added:** "+20 physical damage" — additive with base
- **Increased:** "+15% increased fire damage" — all sum into one bucket, multiply once
- **More:** "20% more damage while moving" — each applied separately, fully multiplicative

Recompute on equipment change, level up, or buff application/expiry. **Never per frame.** Cache the result on the entity.

### Item model

Three layers:

1. **BaseItem** — what is this (Pistol, Shotgun, Leather Vest). Has intrinsic stats, slot, category, and an affix pool ID.
2. **Affix** — a rollable modifier template. Has tiers (T1 best → T6 worst), each gated by item level with a weight and stat roll ranges. Has a `group` (mutual exclusion within a group on one item) and `tags` (for crafting later).
3. **ItemInstance** — what actually dropped. Stores base, ilvl, rarity, rolled prefixes/suffixes, and **the seed**. Always store the seed — it enables compact saves, debugging, shareable item codes, and replay.

### Roll pipeline

```
1. Drop chance check
2. Rarity roll (weighted, modified by player luck/MF)
3. Base roll (weighted, filtered by ilvl + slot)
4. Affix count (determined by rarity: Magic 1-2, Rare 4-6)
5. For each affix slot:
   a. Filter pool: allowed_categories ⊇ base, tier ilvl_required ≤ item ilvl, group not taken, prefix/suffix slot open
   b. Weighted pick of affix
   c. Weighted pick of eligible tier within affix
   d. Roll each stat between min/max
6. Compute display name from prefix + base + suffix fragments
```

### Procgen

Deferred design decision — likely chunk-based with hand-authored room templates stitched together, but TBD. Flow fields for enemy pathing once levels are traversable (compute once per player-tile-change, every enemy reads cheaply).

---

## Conventions

- **Pool everything in hot paths.** No per-frame allocation for projectiles, particles, damage numbers. Pre-size pools.
- **Batch the JS↔wasm boundary.** Expose `tick(dt)` and `get_render_buffer()`, not per-entity calls.
- **Sprite batching:** one draw call per texture atlas per frame. Pack aggressively.
- **Content is data, not code.** Affixes, base items, enemies, drop tables → RON files in `/crates/content`. Hot-reload in dev builds.
- **Determinism:** seeded RNG threaded through all generation. Every drop, every map, every encounter should be reproducible from `(world_seed, event_id)`.
- **Tests:** `core` crate has unit tests for stat math and roll mechanics. These run on native, fast.
- **Error handling:** `Result` + `thiserror` for library errors. `anyhow` only at binary boundaries.

---

## Build this first: the loot simulator

Before any rendering, before procgen, before combat — build the CLI in `/crates/sim` that:

1. Loads affix/base content from RON
2. Takes `--monster-level N --kills M --seed S` as args
3. Runs M drop rolls
4. Dumps results to CSV: `rarity, base, ilvl, affixes, total_dps_estimate, ...`

Eyeball the distributions. Are T1 affixes appearing at the right rate? Is the average rare interesting? Does the curve feel right at ilvl 20 vs 60? Catch tuning problems in seconds instead of hours.

This is the single highest-leverage tool for an ARPG. Probably a day of work once `core` exists.

---

## Initial scope (v1 — playable demo)

Deliberately tight. Expand only after this is fun.

- **Weapons:** 4 archetypes — pistol, shotgun, SMG, rocket launcher
- **Armor:** 3 slots — helm, chest, boots
- **Affixes:** ~20 per category × 4 tiers (skip T5/T6 for now)
- **Rarities:** Normal, Magic, Rare (no uniques in v1)
- **Enemies:** 5 types, BoxHead-style — basic zombie, fast zombie, fat/tank, ranged spitter, swarm rusher
- **Bosses:** 1
- **Biomes:** 1
- **Room templates:** 20
- **Progression:** XP curve to level ~30, simple passive tree (~40 nodes) or skip passive tree for v1

---

## Out of scope (be ruthless)

- Multiplayer / netcode
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

1. macroquad vs Bevy — start with macroquad, but set a tripwire: if we end up reinventing systems/queries, switch.
2. Save format versioning strategy — bump-on-break with migrations, or backwards-compatible? Decide before the first persistent save.
3. Procgen approach — BSP, WFC, chunk-stitched, or hybrid? Defer until combat + loot are proven.
4. Audio stack — `kira` is the current best-in-class for Rust games, works in wasm. Confirm at the point audio matters.