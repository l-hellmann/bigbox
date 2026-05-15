//! Distribution rollups for the loot simulator. Records each drop into
//! in-memory counters; renders three tables (rarity, base, affix × tier).

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Write};

use h2b_core::{Affix, ItemInstance, Rarity};

pub struct Summary {
    drops: u32,
    normal: RaritySum,
    magic: RaritySum,
    rare: RaritySum,
    by_base: BTreeMap<String, u32>,
    by_affix_tier: BTreeMap<(String, u8), AffixTierSum>,
}

#[derive(Default)]
struct RaritySum {
    count: u32,
    sum_affixes: u64,
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
            normal: RaritySum::default(),
            magic: RaritySum::default(),
            rare: RaritySum::default(),
            by_base: BTreeMap::new(),
            by_affix_tier: BTreeMap::new(),
        }
    }

    pub fn record(&mut self, item: &ItemInstance) {
        self.drops += 1;
        let n_affixes = (item.prefixes.len() + item.suffixes.len()) as u64;
        let rarity_sum = match item.rarity {
            Rarity::Normal => &mut self.normal,
            Rarity::Magic => &mut self.magic,
            Rarity::Rare => &mut self.rare,
        };
        rarity_sum.count += 1;
        rarity_sum.sum_affixes += n_affixes;

        *self.by_base.entry(item.base.clone()).or_insert(0) += 1;

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
        for (label, s) in [
            ("Normal", &self.normal),
            ("Magic", &self.magic),
            ("Rare", &self.rare),
        ] {
            let pct = pct_of(s.count as f64, self.drops as f64);
            let avg = if s.count > 0 {
                s.sum_affixes as f64 / s.count as f64
            } else {
                0.0
            };
            writeln!(
                out,
                "{:<10} {:>8} {:>7.2}% {:>14.2}",
                label, s.count, pct, avg
            )?;
        }
        writeln!(out)?;

        writeln!(out, "{:<20} {:>8} {:>8}", "Base", "count", "pct")?;
        for (base, &count) in &self.by_base {
            let pct = pct_of(count as f64, self.drops as f64);
            writeln!(out, "{:<20} {:>8} {:>7.2}%", base, count, pct)?;
        }
        writeln!(out)?;

        writeln!(
            out,
            "{:<24} {:>4} {:>8} {:>10} {:>19} {:>10}",
            "Affix", "tier", "count", "avg_roll", "theoretical_range", "fill_pct"
        )?;
        // BTreeMap iteration groups rows by affix_id, then tier ascending (T1 first).
        // We display the first stat per affix-tier — fine for the current content
        // where every tier rolls a single stat. Extend if multi-stat affixes land.
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
