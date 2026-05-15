//! Distribution rollups for the loot simulator. Records each drop into
//! in-memory counters; renders three tables (rarity, base, affix × tier).

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Write};

use h2b_core::{Affix, ItemInstance, Rarity};

pub struct Summary {
    drops: u32,
    rarities: [RaritySum; Rarity::ALL.len()],
    by_base: BTreeMap<String, BaseSum>,
    by_affix_tier: BTreeMap<(String, u8), AffixTierSum>,
}

#[derive(Default, Clone, Copy)]
struct RaritySum {
    count: u32,
    sum_affixes: u64,
}

#[derive(Default, Clone, Copy)]
struct BaseSum {
    count: u32,
    sum_dps: f64,
    /// TTK is only meaningful for weapons. Track separately so non-weapons
    /// don't drag the average toward zero.
    weapon_count: u32,
    sum_ttk: f64,
    /// Per-rarity TTK breakdown; indexed by `Rarity::index()`.
    ttk_by_rarity: [TtkAccum; Rarity::ALL.len()],
}

#[derive(Default, Clone, Copy)]
struct TtkAccum {
    weapon_count: u32,
    sum_ttk: f64,
}

struct AffixTierSum {
    count: u32,
    rolls: Vec<RollStats>,
}

#[derive(Clone, Copy)]
struct RollStats {
    count: u32,
    sum: f64,
    min: f32,
    max: f32,
}

impl RollStats {
    fn empty() -> Self {
        Self {
            count: 0,
            sum: 0.0,
            min: f32::INFINITY,
            max: f32::NEG_INFINITY,
        }
    }

    fn observe(&mut self, value: f32) {
        self.count += 1;
        self.sum += value as f64;
        if value < self.min {
            self.min = value;
        }
        if value > self.max {
            self.max = value;
        }
    }
}

impl Summary {
    pub fn new() -> Self {
        Self {
            drops: 0,
            rarities: [RaritySum::default(); Rarity::ALL.len()],
            by_base: BTreeMap::new(),
            by_affix_tier: BTreeMap::new(),
        }
    }

    pub fn record(&mut self, item: &ItemInstance, dps: f32, ttk: f32) {
        self.drops += 1;
        let n_affixes = (item.prefixes.len() + item.suffixes.len()) as u64;
        let rs = &mut self.rarities[item.rarity.index()];
        rs.count += 1;
        rs.sum_affixes += n_affixes;

        let base = self.by_base.entry(item.base.clone()).or_default();
        base.count += 1;
        base.sum_dps += dps as f64;
        if ttk > 0.0 {
            base.weapon_count += 1;
            base.sum_ttk += ttk as f64;
            let acc = &mut base.ttk_by_rarity[item.rarity.index()];
            acc.weapon_count += 1;
            acc.sum_ttk += ttk as f64;
        }

        for a in item.prefixes.iter().chain(item.suffixes.iter()) {
            let entry = self
                .by_affix_tier
                .entry((a.affix_id.clone(), a.tier))
                .or_insert(AffixTierSum {
                    count: 0,
                    rolls: Vec::new(),
                });
            entry.count += 1;
            if entry.rolls.len() < a.rolls.len() {
                entry.rolls.resize(a.rolls.len(), RollStats::empty());
            }
            for (i, &r) in a.rolls.iter().enumerate() {
                entry.rolls[i].observe(r);
            }
        }
    }

    pub fn print<W: Write>(
        &self,
        out: &mut W,
        affixes: &[Affix],
        seed: u64,
        ilvl: u32,
    ) -> io::Result<()> {
        let index: HashMap<&str, &Affix> = affixes.iter().map(|a| (a.id.as_str(), a)).collect();

        writeln!(out, "=== head2box-sim summary ===")?;
        writeln!(
            out,
            "seed: {seed}  ilvl: {ilvl}  drops: {drops}",
            drops = self.drops
        )?;
        writeln!(out)?;

        writeln!(
            out,
            "{:<10} {:>8} {:>8} {:>14}",
            "Rarity", "count", "pct", "avg_affixes"
        )?;
        for rarity in Rarity::ALL {
            let s = &self.rarities[rarity.index()];
            let pct = pct_of(s.count as f64, self.drops as f64);
            let avg = if s.count > 0 {
                s.sum_affixes as f64 / s.count as f64
            } else {
                0.0
            };
            writeln!(
                out,
                "{:<10} {:>8} {:>7.2}% {:>14.2}",
                format!("{rarity:?}"),
                s.count,
                pct,
                avg
            )?;
        }
        writeln!(out)?;

        writeln!(
            out,
            "{:<20} {:>8} {:>8} {:>10} {:>10}",
            "Base", "count", "pct", "avg_dps", "avg_ttk"
        )?;
        for (base, sum) in &self.by_base {
            let pct = pct_of(sum.count as f64, self.drops as f64);
            let avg_dps = if sum.count > 0 {
                sum.sum_dps / sum.count as f64
            } else {
                0.0
            };
            let ttk_cell = if sum.weapon_count > 0 {
                format!("{:>10.3}", sum.sum_ttk / sum.weapon_count as f64)
            } else {
                format!("{:>10}", "—")
            };
            writeln!(
                out,
                "{:<20} {:>8} {:>7.2}% {:>10.2} {}",
                base, sum.count, pct, avg_dps, ttk_cell
            )?;
        }
        writeln!(out)?;

        let any_weapon = self.by_base.values().any(|s| s.weapon_count > 0);
        if any_weapon {
            write!(out, "{:<20}", "TTK by rarity")?;
            for rarity in Rarity::ALL {
                write!(out, " {:>10}", format!("{rarity:?}"))?;
            }
            writeln!(out)?;
            for (base, sum) in &self.by_base {
                if sum.weapon_count == 0 {
                    continue;
                }
                write!(out, "{:<20}", base)?;
                for rarity in Rarity::ALL {
                    let acc = &sum.ttk_by_rarity[rarity.index()];
                    if acc.weapon_count > 0 {
                        let avg = acc.sum_ttk / acc.weapon_count as f64;
                        write!(out, " {:>9.3}s", avg)?;
                    } else {
                        write!(out, " {:>10}", "—")?;
                    }
                }
                writeln!(out)?;
            }
            writeln!(out)?;
        }

        writeln!(
            out,
            "{:<24} {:>4} {:>8} {:>10} {:>19} {:>10}",
            "Affix", "tier", "count", "avg_roll", "theoretical_range", "fill_pct"
        )?;
        for ((affix_id, tier), s) in &self.by_affix_tier {
            let (theo_min, theo_max) = lookup_range(&index, affix_id, *tier);
            let rs = s.rolls.first().copied().unwrap_or_else(RollStats::empty);
            let avg = if rs.count > 0 {
                rs.sum / rs.count as f64
            } else {
                0.0
            };
            let fill = if theo_max > theo_min {
                100.0 * (avg - theo_min as f64) / (theo_max - theo_min) as f64
            } else {
                0.0
            };
            writeln!(
                out,
                "{:<24} T{:<3} {:>8} {:>10.3} [{:>7.2}, {:>7.2}] {:>9.1}%",
                affix_id, tier, s.count, avg, theo_min, theo_max, fill
            )?;
        }
        Ok(())
    }
}

fn pct_of(num: f64, denom: f64) -> f64 {
    if denom > 0.0 { 100.0 * num / denom } else { 0.0 }
}

fn lookup_range(index: &HashMap<&str, &Affix>, affix_id: &str, tier: u8) -> (f32, f32) {
    let affix = match index.get(affix_id) {
        Some(a) => a,
        None => return (0.0, 0.0),
    };
    let t = match affix.tiers.iter().find(|t| t.tier == tier) {
        Some(t) => t,
        None => return (0.0, 0.0),
    };
    match t.stats.first() {
        Some(s) => (s.min, s.max),
        None => (0.0, 0.0),
    }
}
