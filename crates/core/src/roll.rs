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

    // Step 3: base — uniform pick (weights can come later when base rarity matters).
    let base = &bases[rng.gen_range(0..bases.len())];

    // Stash the seed *before* rolling affixes so a future re-roll from the seed
    // is reproducible. The current rng state is what we record.
    let seed: u64 = rng.r#gen();

    // Step 4: affix count by rarity.
    let (n_prefix, n_suffix) = affix_counts(rng, rarity);

    let mut taken_groups: Vec<String> = Vec::with_capacity(n_prefix + n_suffix);
    let prefixes = roll_affix_set(
        rng,
        affixes,
        base,
        ilvl,
        AffixSlot::Prefix,
        n_prefix,
        &mut taken_groups,
    );
    let suffixes = roll_affix_set(
        rng,
        affixes,
        base,
        ilvl,
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
        Rarity::Normal => (0, 0),
        Rarity::Magic => {
            // 1 or 2 affixes total; if 1, coinflip prefix vs suffix.
            if rng.gen_bool(0.5) {
                (1, 1)
            } else if rng.gen_bool(0.5) {
                (1, 0)
            } else {
                (0, 1)
            }
        }
        Rarity::Rare => {
            // 4-6 affixes total, capped at 3 per side.
            let total: usize = rng.gen_range(4..=6);
            let n_pre = rng.gen_range(total.saturating_sub(3)..=total.min(3));
            (n_pre, total - n_pre)
        }
    }
}

fn roll_affix_set<R: Rng + ?Sized>(
    rng: &mut R,
    affixes: &[Affix],
    base: &BaseItem,
    ilvl: u32,
    slot: AffixSlot,
    count: usize,
    taken_groups: &mut Vec<String>,
) -> Vec<RolledAffix> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let Some(rolled) = roll_one_affix(rng, affixes, base, ilvl, slot, taken_groups) else {
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
    slot: AffixSlot,
    taken_groups: &[String],
) -> Option<PickedAffix> {
    // Step 5a: filter pool.
    let candidates: Vec<&Affix> = affixes
        .iter()
        .filter(|a| a.slot == slot)
        .filter(|a| !taken_groups.iter().any(|g| g == &a.group))
        .filter(|a| a.allowed_categories.iter().any(|c| c == &base.category))
        .filter(|a| a.tiers.iter().any(|t| t.ilvl_required <= ilvl))
        .collect();
    if candidates.is_empty() {
        return None;
    }

    // Step 5b: weighted pick across affixes. Each affix's effective weight is
    // the sum of its eligible tier weights — so an affix with more eligible
    // tiers gets proportionally more representation.
    let weights: Vec<u32> = candidates
        .iter()
        .map(|a| {
            a.tiers
                .iter()
                .filter(|t| t.ilvl_required <= ilvl)
                .map(|t| t.weight)
                .sum()
        })
        .collect();
    let affix = pick_weighted(rng, &candidates, &weights)?;

    // Step 5c: tier pick within the chosen affix.
    let eligible: Vec<&AffixTier> = affix
        .tiers
        .iter()
        .filter(|t| t.ilvl_required <= ilvl)
        .collect();
    let tier_weights: Vec<u32> = eligible.iter().map(|t| t.weight).collect();
    let tier = pick_weighted(rng, &eligible, &tier_weights)?;

    // Step 5d: roll each stat.
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
            affix_pool: "weapon".into(),
        }];
        let affixes = vec![
            Affix {
                id: "flat_phys".into(),
                name_fragment: "Sharp".into(),
                slot: AffixSlot::Prefix,
                group: "phys_flat".into(),
                tags: vec![],
                allowed_categories: vec!["weapon".into()],
                tiers: vec![AffixTier {
                    tier: 4,
                    ilvl_required: 1,
                    weight: 100,
                    stats: vec![StatRoll {
                        stat: "physical_damage".into(),
                        kind: ModifierKind::Flat,
                        min: 5.0,
                        max: 10.0,
                    }],
                }],
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
    fn normal_has_no_affixes() {
        let (bases, affixes) = fixture();
        let mut rng = StdRng::seed_from_u64(1);
        let item = roll_item(&mut rng, &bases, &affixes, 60, Rarity::Normal).unwrap();
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
        // With only one prefix-eligible affix in the fixture, even a Rare item
        // can have at most 1 prefix — the group exclusion must hold.
        for _ in 0..50 {
            let item = roll_item(&mut rng, &bases, &affixes, 60, Rarity::Rare).unwrap();
            assert!(item.prefixes.len() <= 1);
            assert!(item.suffixes.len() <= 1);
        }
    }
}
