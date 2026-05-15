//! Combine an item's intrinsic stats with its rolled affixes into final
//! per-stat values via the three-tier formula. Single entry point combat
//! and UI will use to read what an item actually contributes.

use std::collections::HashMap;

use crate::affix::Affix;
use crate::item::{BaseItem, ItemInstance};
use crate::stats::{Modifier, StatId, aggregate};
use crate::upgrade::upgrade_scale;

/// Returns `stat_id → final_value` for every stat that has either an intrinsic
/// on `base` or at least one rolled modifier on `item`. Each value is the
/// result of `aggregate(intrinsic_sum, [Modifier; ..])` for that stat.
///
/// Rolled affixes whose `affix_id` or `tier` isn't found in `affixes` are
/// silently skipped — in normal use, items roll from the same affix slice, so
/// a missing lookup indicates a data inconsistency rather than a runtime case
/// we need to surface.
pub fn aggregate_item(
    item: &ItemInstance,
    base: &BaseItem,
    affixes: &[Affix],
) -> HashMap<StatId, f32> {
    let mut bases: HashMap<StatId, f32> = HashMap::new();
    for intrinsic in &base.intrinsic_stats {
        *bases.entry(intrinsic.stat.clone()).or_insert(0.0) += intrinsic.value;
    }

    let mut modifiers: HashMap<StatId, Vec<Modifier>> = HashMap::new();
    for rolled in item.prefixes.iter().chain(item.suffixes.iter()) {
        let Some(affix) = affixes.iter().find(|a| a.id == rolled.affix_id) else {
            continue;
        };
        let Some(tier) = affix.tiers.iter().find(|t| t.tier == rolled.tier) else {
            continue;
        };
        for (stat_roll, &value) in tier.stats.iter().zip(rolled.rolls.iter()) {
            modifiers
                .entry(stat_roll.stat.clone())
                .or_default()
                .push(Modifier {
                    kind: stat_roll.kind,
                    value,
                });
        }
    }

    let mut result: HashMap<StatId, f32> = HashMap::new();
    for (stat, &base_val) in &bases {
        let mods = modifiers.get(stat).map(|m| m.as_slice()).unwrap_or(&[]);
        result.insert(stat.clone(), aggregate(base_val, mods));
    }
    for (stat, mods) in &modifiers {
        if !bases.contains_key(stat) {
            result.insert(stat.clone(), aggregate(0.0, mods));
        }
    }

    // Apply player-side upgrade scaling uniformly. Tier 0 = identity, so
    // fresh drops pass through unchanged.
    let scale = upgrade_scale(item.upgrade_tier);
    if scale != 1.0 {
        for v in result.values_mut() {
            *v *= scale;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::affix::{AffixSlot, AffixTier, StatRoll};
    use crate::item::{IntrinsicStat, Rarity, RolledAffix};
    use crate::stats::ModifierKind;

    fn pistol() -> BaseItem {
        BaseItem {
            id: "pistol".into(),
            name: "Pistol".into(),
            category: "weapon".into(),
            slot: "weapon".into(),
            intrinsic_stats: vec![
                IntrinsicStat {
                    stat: "weapon_damage".into(),
                    value: 12.0,
                },
                IntrinsicStat {
                    stat: "fire_rate".into(),
                    value: 4.0,
                },
            ],
        }
    }

    fn empty_instance(base_id: &str) -> ItemInstance {
        ItemInstance {
            base: base_id.into(),
            ilvl: 60,
            rarity: Rarity::Basic,
            seed: 0,
            prefixes: vec![],
            suffixes: vec![],
            upgrade_tier: 0,
        }
    }

    fn affix_one_tier(
        id: &str,
        slot: AffixSlot,
        stat: &str,
        kind: ModifierKind,
        min: f32,
        max: f32,
    ) -> Affix {
        Affix {
            id: id.into(),
            name_fragment: id.into(),
            slot,
            group: id.into(),
            tags: vec![],
            allowed_categories: vec!["weapon".into()],
            tiers: vec![AffixTier {
                tier: 1,
                ilvl_required: 1,
                weight: 100,
                stats: vec![StatRoll {
                    stat: stat.into(),
                    kind,
                    min,
                    max,
                }],
            }],
        }
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn no_affixes_returns_intrinsics() {
        let base = pistol();
        let item = empty_instance("pistol");
        let result = aggregate_item(&item, &base, &[]);
        assert_eq!(result.get("weapon_damage"), Some(&12.0));
        assert_eq!(result.get("fire_rate"), Some(&4.0));
    }

    #[test]
    fn flat_affix_on_zero_base() {
        let base = pistol();
        let mut item = empty_instance("pistol");
        item.prefixes.push(RolledAffix {
            affix_id: "flat_bullet".into(),
            tier: 1,
            rolls: vec![10.0],
        });
        let affixes = vec![affix_one_tier(
            "flat_bullet",
            AffixSlot::Prefix,
            "bullet_damage",
            ModifierKind::Flat,
            10.0,
            10.0,
        )];
        let result = aggregate_item(&item, &base, &affixes);
        assert_eq!(result.get("bullet_damage"), Some(&10.0));
    }

    #[test]
    fn increased_affix_scales_intrinsic_base() {
        let base = pistol();
        let mut item = empty_instance("pistol");
        item.prefixes.push(RolledAffix {
            affix_id: "inc_weapon".into(),
            tier: 1,
            rolls: vec![0.50],
        });
        let affixes = vec![affix_one_tier(
            "inc_weapon",
            AffixSlot::Prefix,
            "weapon_damage",
            ModifierKind::Increased,
            0.50,
            0.50,
        )];
        let result = aggregate_item(&item, &base, &affixes);
        // (12 + 0) × (1 + 0.50) × 1 = 18
        let got = *result.get("weapon_damage").unwrap();
        assert!(approx(got, 18.0), "got {got}");
    }

    #[test]
    fn three_tier_formula_per_stat() {
        let base = BaseItem {
            id: "x".into(),
            name: "X".into(),
            category: "weapon".into(),
            slot: "weapon".into(),
            intrinsic_stats: vec![IntrinsicStat {
                stat: "x".into(),
                value: 100.0,
            }],
        };
        let item = ItemInstance {
            base: "x".into(),
            ilvl: 60,
            rarity: Rarity::Rare,
            seed: 0,
            prefixes: vec![
                RolledAffix {
                    affix_id: "flat".into(),
                    tier: 1,
                    rolls: vec![20.0],
                },
                RolledAffix {
                    affix_id: "inc".into(),
                    tier: 1,
                    rolls: vec![0.25],
                },
            ],
            suffixes: vec![RolledAffix {
                affix_id: "more".into(),
                tier: 1,
                rolls: vec![0.20],
            }],
            upgrade_tier: 0,
        };
        let affixes = vec![
            affix_one_tier("flat", AffixSlot::Prefix, "x", ModifierKind::Flat, 20.0, 20.0),
            affix_one_tier("inc", AffixSlot::Prefix, "x", ModifierKind::Increased, 0.25, 0.25),
            affix_one_tier("more", AffixSlot::Suffix, "x", ModifierKind::More, 0.20, 0.20),
        ];
        let result = aggregate_item(&item, &base, &affixes);
        // (100 + 20) × (1 + 0.25) × (1 + 0.20) = 180
        let got = *result.get("x").unwrap();
        assert!(approx(got, 180.0), "got {got}");
    }

    #[test]
    fn upgrade_tier_scales_all_aggregated_stats() {
        let base = pistol();
        let mut item = empty_instance("pistol");
        item.prefixes.push(RolledAffix {
            affix_id: "flat_bullet".into(),
            tier: 1,
            rolls: vec![10.0],
        });
        let affixes = vec![affix_one_tier(
            "flat_bullet",
            AffixSlot::Prefix,
            "bullet_damage",
            ModifierKind::Flat,
            10.0,
            10.0,
        )];

        let base_result = aggregate_item(&item, &base, &affixes);
        let base_dmg = *base_result.get("weapon_damage").unwrap();
        let base_bullet = *base_result.get("bullet_damage").unwrap();

        // Tier 5 = +40% scaling.
        item.upgrade_tier = 5;
        let upgraded = aggregate_item(&item, &base, &affixes);
        let up_dmg = *upgraded.get("weapon_damage").unwrap();
        let up_bullet = *upgraded.get("bullet_damage").unwrap();

        assert!(approx(up_dmg / base_dmg, 1.40), "weapon_damage ratio");
        assert!(approx(up_bullet / base_bullet, 1.40), "bullet_damage ratio");
    }

    #[test]
    fn affixes_on_different_stats_are_independent() {
        let base = pistol();
        let mut item = empty_instance("pistol");
        item.suffixes.push(RolledAffix {
            affix_id: "rate".into(),
            tier: 1,
            rolls: vec![0.10],
        });
        let affixes = vec![affix_one_tier(
            "rate",
            AffixSlot::Suffix,
            "fire_rate",
            ModifierKind::More,
            0.10,
            0.10,
        )];
        let result = aggregate_item(&item, &base, &affixes);
        // weapon_damage unchanged; fire_rate 4 × 1.10 = 4.4
        assert_eq!(result.get("weapon_damage"), Some(&12.0));
        let rate = *result.get("fire_rate").unwrap();
        assert!(approx(rate, 4.4), "got {rate}");
    }
}
