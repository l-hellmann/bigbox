//! Spawn placement — picks Floor tiles for placing enemies (or anything
//! else) on a generated map. Built on top of `FlowField`: every candidate
//! tile is reachable from the goal by construction (any unreachable tile
//! has `UNREACHABLE` distance and is filtered out).
//!
//! Weighting is **linear in distance from the goal** — farther tiles are
//! proportionally more likely to be picked. This avoids the "swarm of
//! zombies on top of the player" failure mode without doing anything
//! fancy; for stronger biasing, square the weights or band-filter the
//! distances before calling.

use rand::Rng;

use crate::flow::{FlowField, UNREACHABLE};

/// Pick up to `count` distinct Floor tiles from `field`, all at distance
/// `>= min_distance` from the goal, weighted linearly by distance.
///
/// Returns fewer than `count` if not enough candidates exist. Caller-side
/// rng controls determinism.
pub fn pick_spawn_points<R: Rng + ?Sized>(
    rng: &mut R,
    field: &FlowField,
    count: usize,
    min_distance: u32,
) -> Vec<(u32, u32)> {
    let mut candidates: Vec<((u32, u32), u32)> =
        Vec::with_capacity((field.width * field.height) as usize / 4);
    for y in 0..field.height {
        for x in 0..field.width {
            let d = field.distance_at(x, y);
            if d != UNREACHABLE && d >= min_distance {
                candidates.push(((x, y), d));
            }
        }
    }

    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if candidates.is_empty() {
            break;
        }
        let total: u64 = candidates.iter().map(|&(_, w)| w as u64).sum();
        if total == 0 {
            // All remaining candidates have weight 0 — fall back to uniform.
            let idx = rng.gen_range(0..candidates.len());
            let (pt, _) = candidates.swap_remove(idx);
            out.push(pt);
            continue;
        }
        let mut pick = rng.gen_range(0..total);
        let mut chosen = 0;
        for (i, &(_, w)) in candidates.iter().enumerate() {
            let wu = w as u64;
            if pick < wu {
                chosen = i;
                break;
            }
            pick -= wu;
        }
        let (pt, _) = candidates.swap_remove(chosen);
        out.push(pt);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MapParams, generate_bsp};
    use rand::{SeedableRng, rngs::StdRng};

    fn bsp_field(seed: u64) -> (crate::Map, FlowField) {
        let map = generate_bsp(&MapParams {
            seed,
            ..Default::default()
        });
        let ff = FlowField::compute(&map, map.player_spawn);
        (map, ff)
    }

    #[test]
    fn all_picks_respect_min_distance() {
        let (_, ff) = bsp_field(42);
        let mut rng = StdRng::seed_from_u64(1);
        let picks = pick_spawn_points(&mut rng, &ff, 20, 8);
        assert!(!picks.is_empty(), "BSP map should have enough far tiles");
        for (x, y) in &picks {
            assert!(
                ff.distance_at(*x, *y) >= 8,
                "pick ({x},{y}) is at distance {} but min was 8",
                ff.distance_at(*x, *y)
            );
        }
    }

    #[test]
    fn picks_are_all_reachable() {
        let (_, ff) = bsp_field(42);
        let mut rng = StdRng::seed_from_u64(2);
        let picks = pick_spawn_points(&mut rng, &ff, 30, 0);
        for (x, y) in picks {
            assert_ne!(ff.distance_at(x, y), UNREACHABLE);
        }
    }

    #[test]
    fn no_duplicate_picks() {
        let (_, ff) = bsp_field(42);
        let mut rng = StdRng::seed_from_u64(3);
        let picks = pick_spawn_points(&mut rng, &ff, 50, 0);
        let mut sorted = picks.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), picks.len(), "got duplicate spawn points");
    }

    #[test]
    fn count_caps_at_available_candidates() {
        let (_, ff) = bsp_field(42);
        let mut rng = StdRng::seed_from_u64(4);
        // Ask for absurdly many — should bound to candidates available.
        let picks = pick_spawn_points(&mut rng, &ff, 100_000, 0);
        let total_floor: usize = {
            let ff_ref = &ff;
            (0..ff.height)
                .flat_map(|y| (0..ff_ref.width).map(move |x| (x, y)))
                .filter(|&(x, y)| ff_ref.distance_at(x, y) != UNREACHABLE)
                .count()
        };
        assert_eq!(picks.len(), total_floor);
    }

    #[test]
    fn determinism_same_seed_same_picks() {
        let (_, ff) = bsp_field(42);
        let mut a = StdRng::seed_from_u64(7);
        let mut b = StdRng::seed_from_u64(7);
        let pa = pick_spawn_points(&mut a, &ff, 20, 5);
        let pb = pick_spawn_points(&mut b, &ff, 20, 5);
        assert_eq!(pa, pb);
    }

    #[test]
    fn linear_weighting_biases_picks_toward_far() {
        // Over many seeds, the mean distance of *picks* should exceed the
        // mean distance of *all eligible tiles* — that's the weighting
        // doing what it's supposed to.
        let (_, ff) = bsp_field(42);

        let all_distances: Vec<u32> = {
            let ff_ref = &ff;
            (0..ff.height)
                .flat_map(|y| (0..ff_ref.width).map(move |x| ff_ref.distance_at(x, y)))
                .filter(|&d| d != UNREACHABLE)
                .collect()
        };
        let baseline_mean: f64 =
            all_distances.iter().map(|&d| d as f64).sum::<f64>() / all_distances.len() as f64;

        let mut total = 0u64;
        let mut n = 0u64;
        for seed in 0..50 {
            let mut rng = StdRng::seed_from_u64(seed);
            let picks = pick_spawn_points(&mut rng, &ff, 10, 0);
            for (x, y) in picks {
                total += ff.distance_at(x, y) as u64;
                n += 1;
            }
        }
        let picked_mean = total as f64 / n as f64;
        assert!(
            picked_mean > baseline_mean,
            "weighting should bias picks toward larger distance: \
             picked_mean = {picked_mean:.2}, baseline_mean = {baseline_mean:.2}"
        );
    }
}
