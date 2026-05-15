//! Roll pipeline. See `CLAUDE.md` "Roll pipeline" for the canonical 6-step flow.
//! Threads an explicit [`rand::Rng`] so every drop is reproducible from a seed.

use rand::Rng;
use thiserror::Error;

use crate::affix::{Affix, AffixSlot, AffixTier};
use crate::item::{BaseItem, ItemInstance, Rarity, RolledAffix};

#[derive(Debug, Error)]
pub enum RollError {
    #[error("no base items provided")]
    NoBases,
}

pub fn roll_item<R: Rng + ?Sized>(
    rng: &mut R,
    bases: &[BaseItem],
    affixes: &[Affix],
    ilvl: u32,
    rarity: Rarity,
) -> Result<ItemInstance, RollError> {
    if bases.is_empty() {
        return Err(RollError::NoBases);
    }

    let base = &bases[rng.gen_range(0..bases.len())];
    let seed: u64 = rng.r#gen();

    let (n_prefix, n_suffix) = affix_counts(rng, rarity);
    let tier_floor = rarity.tier_floor();

    let mut taken_groups: Vec<String> = Vec::with_capacity(n_prefix + n_suffix);
    let prefixes = roll_affix_set(
        rng,
        affixes,
        base,
        ilvl,
        tier_floor,
        AffixSlot::Prefix,
        n_prefix,
        &mut taken_groups,
    );
    let suffixes = roll_affix_set(
        rng,
        affixes,
        base,
        ilvl,
        tier_floor,
        AffixSlot::Suffix,
        n_suffix,
        &mut taken_groups,
    );

    Ok(ItemInstance {
        base: base.id.clone(),
        ilvl,
        rarity,
        seed,
        prefixes,
        suffixes,
    })
}

fn affix_counts<R: Rng + ?Sized>(rng: &mut R, rarity: Rarity) -> (usize, usize) {
    match rarity {
        Rarity::Basic => (0, 0),
        Rarity::Common => {
            if rng.gen_bool(0.5) {
                (1, 1)
            } else if rng.gen_bool(0.5) {
                (1, 0)
            } else {
                (0, 1)
            }
        }
        Rarity::Rare => {
            let total = rng.gen_range(3..=4_usize);
            split_total(rng, total)
        }
        Rarity::Epic => {
            let total = rng.gen_range(4..=5_usize);
            split_total(rng, total)
        }
        Rarity::Legendary => {
            let total = rng.gen_range(5..=6_usize);
            split_total(rng, total)
        }
    }
}

fn split_total<R: Rng + ?Sized>(rng: &mut R, total: usize) -> (usize, usize) {
    // Cap each side at 3 (prefix/suffix limit).
    let lo = total.saturating_sub(3);
    let hi = total.min(3);
    let n_pre = rng.gen_range(lo..=hi);
    (n_pre, total - n_pre)
}

fn is_tier_eligible(t: &AffixTier, ilvl: u32, tier_floor: u8) -> bool {
    t.ilvl_required <= ilvl && t.tier <= tier_floor
}

fn roll_affix_set<R: Rng + ?Sized>(
    rng: &mut R,
    affixes: &[Affix],
    base: &BaseItem,
    ilvl: u32,
    tier_floor: u8,
    slot: AffixSlot,
    count: usize,
    taken_groups: &mut Vec<String>,
) -> Vec<RolledAffix> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let Some(rolled) =
            roll_one_affix(rng, affixes, base, ilvl, tier_floor, slot, taken_groups)
        else {
            break;
        };
        taken_groups.push(rolled.group.clone());
        out.push(rolled.rolled);
    }
    out
}

struct PickedAffix {
    group: String,
    rolled: RolledAffix,
}

fn roll_one_affix<R: Rng + ?Sized>(
    rng: &mut R,
    affixes: &[Affix],
    base: &BaseItem,
    ilvl: u32,
    tier_floor: u8,
    slot: AffixSlot,
    taken_groups: &[String],
) -> Option<PickedAffix> {
    let candidates: Vec<&Affix> = affixes
        .iter()
        .filter(|a| a.slot == slot)
        .filter(|a| !taken_groups.iter().any(|g| g == &a.group))
        .filter(|a| a.allowed_categories.iter().any(|c| c == &base.category))
        .filter(|a| {
            a.tiers
                .iter()
                .any(|t| is_tier_eligible(t, ilvl, tier_floor))
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }

    let weights: Vec<u32> = candidates
        .iter()
        .map(|a| {
            a.tiers
                .iter()
                .filter(|t| is_tier_eligible(t, ilvl, tier_floor))
                .map(|t| t.weight)
                .sum()
        })
        .collect();
    let affix = pick_weighted(rng, &candidates, &weights)?;

    let eligible: Vec<&AffixTier> = affix
        .tiers
        .iter()
        .filter(|t| is_tier_eligible(t, ilvl, tier_floor))
        .collect();
    let tier_weights: Vec<u32> = eligible.iter().map(|t| t.weight).collect();
    let tier = pick_weighted(rng, &eligible, &tier_weights)?;

    let rolls: Vec<f32> = tier
        .stats
        .iter()
        .map(|s| s.min + rng.r#gen::<f32>() * (s.max - s.min))
        .collect();

    Some(PickedAffix {
        group: affix.group.clone(),
        rolled: RolledAffix {
            affix_id: affix.id.clone(),
            tier: tier.tier,
            rolls,
        },
    })
}

fn pick_weighted<R: Rng + ?Sized, T: Copy>(rng: &mut R, items: &[T], weights: &[u32]) -> Option<T> {
    let total: u32 = weights.iter().sum();
    if total == 0 {
        return None;
    }
    let mut pick = rng.gen_range(0..total);
    for (i, w) in weights.iter().enumerate() {
        if pick < *w {
            return Some(items[i]);
        }
        pick -= *w;
    }
    items.last().copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::affix::StatRoll;
    use crate::stats::ModifierKind;
    use rand::{SeedableRng, rngs::StdRng};

    fn fixture() -> (Vec<BaseItem>, Vec<Affix>) {
        let bases = vec![BaseItem {
            id: "pistol".into(),
            name: "Pistol".into(),
            category: "weapon".into(),
            slot: "weapon".into(),
            intrinsic_stats: vec![],
        }];
        let affixes = vec![
            Affix {
                id: "flat_phys".into(),
                name_fragment: "Sharp".into(),
                slot: AffixSlot::Prefix,
                group: "phys_flat".into(),
                tags: vec![],
                allowed_categories: vec!["weapon".into()],
                tiers: vec![
                    AffixTier {
                        tier: 4,
                        ilvl_required: 1,
                        weight: 100,
                        stats: vec![StatRoll {
                            stat: "physical_damage".into(),
                            kind: ModifierKind::Flat,
                            min: 5.0,
                            max: 10.0,
                        }],
                    },
                    AffixTier {
                        tier: 2,
                        ilvl_required: 40,
                        weight: 100,
                        stats: vec![StatRoll {
                            stat: "physical_damage".into(),
                            kind: ModifierKind::Flat,
                            min: 15.0,
                            max: 20.0,
                        }],
                    },
                ],
            },
            Affix {
                id: "atk_speed".into(),
                name_fragment: "of Speed".into(),
                slot: AffixSlot::Suffix,
                group: "atk_speed".into(),
                tags: vec![],
                allowed_categories: vec!["weapon".into()],
                tiers: vec![AffixTier {
                    tier: 4,
                    ilvl_required: 1,
                    weight: 100,
                    stats: vec![StatRoll {
                        stat: "attack_speed".into(),
                        kind: ModifierKind::More,
                        min: 0.05,
                        max: 0.10,
                    }],
                }],
            },
        ];
        (bases, affixes)
    }

    #[test]
    fn basic_has_no_affixes() {
        let (bases, affixes) = fixture();
        let mut rng = StdRng::seed_from_u64(1);
        let item = roll_item(&mut rng, &bases, &affixes, 60, Rarity::Basic).unwrap();
        assert_eq!(item.prefixes.len(), 0);
        assert_eq!(item.suffixes.len(), 0);
    }

    #[test]
    fn same_seed_same_result() {
        let (bases, affixes) = fixture();
        let mut a = StdRng::seed_from_u64(42);
        let mut b = StdRng::seed_from_u64(42);
        let ia = roll_item(&mut a, &bases, &affixes, 60, Rarity::Rare).unwrap();
        let ib = roll_item(&mut b, &bases, &affixes, 60, Rarity::Rare).unwrap();
        assert_eq!(ia.base, ib.base);
        assert_eq!(ia.seed, ib.seed);
        assert_eq!(ia.prefixes.len(), ib.prefixes.len());
        assert_eq!(ia.suffixes.len(), ib.suffixes.len());
        for (x, y) in ia.prefixes.iter().zip(&ib.prefixes) {
            assert_eq!(x.affix_id, y.affix_id);
            assert_eq!(x.tier, y.tier);
            assert_eq!(x.rolls, y.rolls);
        }
    }

    #[test]
    fn rare_respects_group_exclusion() {
        let (bases, affixes) = fixture();
        let mut rng = StdRng::seed_from_u64(7);
        // Only one prefix-eligible and one suffix-eligible group in the fixture.
        for _ in 0..50 {
            let item = roll_item(&mut rng, &bases, &affixes, 60, Rarity::Rare).unwrap();
            assert!(item.prefixes.len() <= 1);
            assert!(item.suffixes.len() <= 1);
        }
    }

    #[test]
    fn legendary_only_rolls_at_or_above_floor() {
        // Legendary floor = T2; fixture's prefix affix has T2 and T4 — only T2
        // should ever roll. Suffix affix has T4 only, so no suffix should roll.
        let (bases, affixes) = fixture();
        let mut rng = StdRng::seed_from_u64(99);
        let mut saw_any = false;
        for _ in 0..100 {
            let item = roll_item(&mut rng, &bases, &affixes, 60, Rarity::Legendary).unwrap();
            for a in item.prefixes.iter().chain(item.suffixes.iter()) {
                saw_any = true;
                assert!(a.tier <= 2, "tier {} below Legendary floor 2", a.tier);
            }
            assert_eq!(item.suffixes.len(), 0, "no T2-or-better suffix exists");
        }
        assert!(saw_any, "expected at least one affix to roll");
    }
}
