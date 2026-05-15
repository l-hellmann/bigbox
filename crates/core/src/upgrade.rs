//! Item upgrade economy — disenchant other drops into scrap, spend scrap to
//! bump an item's `upgrade_tier`. Diablo-shaped: upgrades amplify what a
//! drop already rolled, they don't change its identity. Drops always start
//! at tier 0; upgrades are a player action layered on top.

use crate::item::Rarity;

pub const MAX_UPGRADE_TIER: u8 = 5;

/// Multiplicative scaling applied to every aggregated stat at `tier`.
/// Linear curve: +8% per tier, so a fully-upgraded item is 1.40× its
/// fresh-drop output. Applied uniformly across all stats — the v1
/// simplification accepts that utility stats (life_regen, movement_speed)
/// scale alongside damage; refine when content needs it.
pub fn upgrade_scale(tier: u8) -> f32 {
    1.0 + 0.08 * tier.min(MAX_UPGRADE_TIER) as f32
}

/// Scrap recovered when disenchanting an item of this rarity. Does **not**
/// refund any scrap already spent on upgrades — players learn not to
/// over-invest in items they don't plan to keep.
pub fn disenchant_value(rarity: Rarity) -> u32 {
    match rarity {
        Rarity::Basic => 1,
        Rarity::Common => 3,
        Rarity::Rare => 10,
        Rarity::Epic => 30,
        Rarity::Legendary => 100,
    }
}

/// Scrap required to advance from `current_tier` to `current_tier + 1`.
/// Returns `None` when already at `MAX_UPGRADE_TIER` — no upgrade left.
///
/// Curve is geometric so the last tier feels earned: 5 / 15 / 40 / 100 / 250.
/// Total to fully upgrade: 410 scrap (≈ 4 Legendaries, ≈ 14 Rares,
/// ≈ 137 Commons disenchanted).
pub fn upgrade_cost(current_tier: u8) -> Option<u32> {
    const COSTS: [u32; MAX_UPGRADE_TIER as usize] = [5, 15, 40, 100, 250];
    COSTS.get(current_tier as usize).copied()
}

/// Total scrap to upgrade from `from` to `to` (inclusive of `to`'s cost).
/// Returns `None` if either bound is out of range.
pub fn total_upgrade_cost(from: u8, to: u8) -> Option<u32> {
    if to > MAX_UPGRADE_TIER || from > to {
        return None;
    }
    let mut total = 0u32;
    for t in from..to {
        total += upgrade_cost(t)?;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_at_tier_zero_is_identity() {
        assert_eq!(upgrade_scale(0), 1.0);
    }

    #[test]
    fn scale_at_max_tier_is_1_4() {
        assert!((upgrade_scale(MAX_UPGRADE_TIER) - 1.40).abs() < 1e-4);
    }

    #[test]
    fn scale_clamps_above_max() {
        // Defensive: anything above MAX clamps, doesn't blow up.
        assert_eq!(upgrade_scale(99), upgrade_scale(MAX_UPGRADE_TIER));
    }

    #[test]
    fn disenchant_values_monotonic_by_rarity() {
        let v: Vec<u32> = Rarity::ALL
            .iter()
            .map(|r| disenchant_value(*r))
            .collect();
        for w in v.windows(2) {
            assert!(w[1] > w[0], "disenchant not monotonic: {v:?}");
        }
    }

    #[test]
    fn upgrade_cost_geometric_and_capped() {
        assert_eq!(upgrade_cost(0), Some(5));
        assert_eq!(upgrade_cost(1), Some(15));
        assert_eq!(upgrade_cost(2), Some(40));
        assert_eq!(upgrade_cost(3), Some(100));
        assert_eq!(upgrade_cost(4), Some(250));
        assert_eq!(upgrade_cost(MAX_UPGRADE_TIER), None);
        // Each step strictly more expensive than the last.
        for t in 0..MAX_UPGRADE_TIER - 1 {
            assert!(upgrade_cost(t).unwrap() < upgrade_cost(t + 1).unwrap());
        }
    }

    #[test]
    fn total_upgrade_cost_zero_to_max_is_410() {
        assert_eq!(total_upgrade_cost(0, MAX_UPGRADE_TIER), Some(410));
    }

    #[test]
    fn total_upgrade_cost_partial_path() {
        // 0 → 3 = 5 + 15 + 40 = 60
        assert_eq!(total_upgrade_cost(0, 3), Some(60));
        // 2 → 4 = 40 + 100 = 140
        assert_eq!(total_upgrade_cost(2, 4), Some(140));
        // Same tier = 0 cost (allowed).
        assert_eq!(total_upgrade_cost(3, 3), Some(0));
        // Invalid bounds.
        assert_eq!(total_upgrade_cost(3, 2), None);
        assert_eq!(total_upgrade_cost(0, MAX_UPGRADE_TIER + 1), None);
    }
}
