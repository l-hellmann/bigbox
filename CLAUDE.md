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

- **Flat added:** "+20 bullet damage" — additive with base
- **Increased:** "+15% increased weapon damage" — all sum into one bucket, multiply once
- **More:** "20% more damage while moving" — each applied separately, fully multiplicative

Recompute on equipment change, level up, or buff application/expiry. **Never per frame.** Cache the result on the entity.

### Item model

Three layers:

1. **BaseItem** — what is this (Pistol, Combat Helmet, …). Has intrinsic stats (per-shot damage, fire rate, base life/armor/evasion, …), a `slot` (where it equips), and a `category` used to filter eligible affixes. Weapon categories split by archetype — `rapid_fire` (pistol, SMG) vs `heavy` (shotgun, rocket launcher) — so flat-damage affix ranges can be calibrated per class (a +10 flat bullet roll is 100 DPS on an SMG vs 5 DPS on a rocket — content compensates instead of trying to express it through a single formula).
2. **Affix** — a rollable modifier template. Has tiers (T1 best → T6 worst), each gated by item level with a weight and stat roll ranges. Has a `group` (mutual exclusion within a group on one item) and `tags` (for crafting later).
3. **ItemInstance** — what actually dropped. Stores base, ilvl, rarity, rolled prefixes/suffixes, and **the seed**. Always store the seed — it enables compact saves, debugging, shareable item codes, and replay.

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

Cap each side at 3 prefixes / 3 suffixes — the affix count above splits across both. At low ilvl, the tier floor may exceed what's currently eligible (e.g. Legendary at ilvl 20 has no T2 affixes available); for v1 the sim is allowed to surface that as "rolled fewer affixes than the band," and we'll add an ilvl-gated rarity downgrade once it actually bites.

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

- **Weapons:** 4 archetypes in 2 categories — `rapid_fire` (pistol, SMG) and `heavy` (shotgun, rocket launcher). Category drives damage-affix calibration; see BaseItem under Core systems.
- **Armor:** 3 slots — helm, chest, boots
- **Affixes:** ~20 per category × 4 tiers (skip T5/T6 for now)
- **Rarities:** 5-tier (Basic / Common / Rare / Epic / Legendary) — see Rarities under Core systems. Standalone uniques deferred.
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