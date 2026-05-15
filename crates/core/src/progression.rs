//! Character progression — the XP curve and helpers around it. Player-side
//! state (current level, total XP) lives in the gameplay layer; `core` just
//! owns the math so the curve can be tested and previewed without dragging
//! in the runtime.
//!
//! Curve shape (CLAUDE.md v1 scope says level ~30, no passive tree yet):
//!   xp_to_reach(L) = (L - 1) × L × 50
//! Cumulative; level 1 is the starting point at 0 XP. Reaches ~43.5k XP at
//! `MAX_LEVEL`, which translates to ~4.3k basic_zombie kills, ~870 fat_zombie
//! kills, or ~87 Patient Zero kills — wide range so the curve feels right
//! across the early/mid/late game.

pub const MAX_LEVEL: u8 = 30;

/// Total XP required to reach `level`, cumulative from level 1.
/// Level 1 returns 0 (starting point). Caps at `xp_to_reach(MAX_LEVEL)` for
/// any level beyond.
pub fn xp_to_reach(level: u8) -> u64 {
    let lvl = level.min(MAX_LEVEL) as u64;
    if lvl <= 1 {
        return 0;
    }
    (lvl - 1) * lvl * 50
}

/// XP needed to advance from `level` to `level + 1`.
/// Returns `None` at `MAX_LEVEL` — no advancement left.
pub fn xp_for_next_level(level: u8) -> Option<u64> {
    if level >= MAX_LEVEL {
        return None;
    }
    Some(xp_to_reach(level + 1) - xp_to_reach(level))
}

/// What level corresponds to having accumulated `total_xp`. Clamped to
/// `[1, MAX_LEVEL]`. Round-trips with `xp_to_reach`: feeding the output of
/// `xp_to_reach(N)` back here yields `N` (for `N <= MAX_LEVEL`).
pub fn level_for_total_xp(total_xp: u64) -> u8 {
    let mut level: u8 = 1;
    while level < MAX_LEVEL && xp_to_reach(level + 1) <= total_xp {
        level += 1;
    }
    level
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_1_starts_at_zero_xp() {
        assert_eq!(xp_to_reach(1), 0);
    }

    #[test]
    fn xp_to_reach_is_monotonic_and_concrete() {
        // Eyeball a few checkpoints so the curve shape doesn't drift silently.
        assert_eq!(xp_to_reach(2), 100); // 1 × 2 × 50
        assert_eq!(xp_to_reach(5), 1000); // 4 × 5 × 50
        assert_eq!(xp_to_reach(10), 4500); // 9 × 10 × 50
        assert_eq!(xp_to_reach(30), 43_500); // 29 × 30 × 50

        let mut prev = 0;
        for lvl in 1..=MAX_LEVEL {
            let xp = xp_to_reach(lvl);
            assert!(xp >= prev, "non-monotonic at level {lvl}");
            prev = xp;
        }
    }

    #[test]
    fn xp_to_reach_clamps_above_max_level() {
        assert_eq!(xp_to_reach(MAX_LEVEL), xp_to_reach(99));
    }

    #[test]
    fn xp_for_next_level_caps_at_max() {
        assert_eq!(xp_for_next_level(MAX_LEVEL), None);
        assert_eq!(xp_for_next_level(MAX_LEVEL - 1), Some(2900)); // 29×30×50 - 28×29×50
        assert_eq!(xp_for_next_level(1), Some(100));
    }

    #[test]
    fn level_for_total_xp_clamps_to_range() {
        assert_eq!(level_for_total_xp(0), 1);
        assert_eq!(level_for_total_xp(99), 1); // not enough for level 2
        assert_eq!(level_for_total_xp(100), 2); // exactly enough
        assert_eq!(level_for_total_xp(u64::MAX), MAX_LEVEL);
    }

    #[test]
    fn round_trip_through_curve() {
        for lvl in 1..=MAX_LEVEL {
            let xp = xp_to_reach(lvl);
            assert_eq!(level_for_total_xp(xp), lvl, "round-trip failed at {lvl}");
        }
    }

    #[test]
    fn one_xp_below_threshold_stays_at_previous_level() {
        // Right at level 5's threshold (1000 XP) → level 5.
        // One short → still level 4.
        assert_eq!(level_for_total_xp(1000), 5);
        assert_eq!(level_for_total_xp(999), 4);
    }
}
