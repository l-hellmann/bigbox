//! Distribution rollups for the loot simulator. Records each drop into
//! in-memory counters; renders three tables (rarity, base, affix × tier).

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Write};

use h2b_core::{Affix, Enemy, ItemInstance, Rarity, progression};

pub struct Summary {
    drops: u32,
    rarities: [RaritySum; Rarity::ALL.len()],
    /// Number of enemies the matrix will display — set at construction
    /// from the loaded enemies slice. Per-base per-enemy TTK accumulators
    /// inside `BaseSum` are sized to match.
    enemy_count: usize,
    by_base: BTreeMap<String, BaseSum>,
    by_affix_tier: BTreeMap<(String, u8), AffixTierSum>,
}

#[derive(Default, Clone, Copy)]
struct RaritySum {
    count: u32,
    sum_affixes: u64,
}

#[derive(Default, Clone)]
struct BaseSum {
    count: u32,
    sum_dps: f64,
    /// TTK is only meaningful for weapons. Track separately so non-weapons
    /// don't drag the average toward zero.
    weapon_count: u32,
    sum_ttk: f64,
    /// "Kitted" = drop + the base's optimal attachment loadout. Same
    /// weapon_count gate as ttk — armor pieces don't contribute.
    sum_kit_dps: f64,
    sum_kit_ttk: f64,
    /// Per-rarity TTK breakdown; indexed by `Rarity::index()`.
    ttk_by_rarity: [TtkAccum; Rarity::ALL.len()],
    /// Per-enemy TTK breakdown; one entry per enemy in load order, sized
    /// when the parent `Summary` is constructed.
    ttk_by_enemy: Vec<TtkAccum>,
}

/// Caller-supplied metrics for a single drop. The sim already knows how to
/// compute these; Summary just records and aggregates.
pub struct DropMetrics {
    pub dps: f32,
    pub ttk: f32,
    pub kit_dps: f32,
    pub kit_ttk: f32,
    /// TTK against each enemy in load order — same length as the enemies
    /// slice passed to `Summary::new`. Drop-only (no attachments).
    pub ttk_per_enemy: Vec<f32>,
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
    pub fn new(enemies: &[Enemy]) -> Self {
        Self {
            drops: 0,
            rarities: [RaritySum::default(); Rarity::ALL.len()],
            enemy_count: enemies.len(),
            by_base: BTreeMap::new(),
            by_affix_tier: BTreeMap::new(),
        }
    }

    fn fresh_base_sum(&self) -> BaseSum {
        BaseSum {
            ttk_by_enemy: vec![TtkAccum::default(); self.enemy_count],
            ..Default::default()
        }
    }

    pub fn record(&mut self, item: &ItemInstance, m: &DropMetrics) {
        self.drops += 1;
        let n_affixes = (item.prefixes.len() + item.suffixes.len()) as u64;
        let rs = &mut self.rarities[item.rarity.index()];
        rs.count += 1;
        rs.sum_affixes += n_affixes;

        let seed = self.fresh_base_sum();
        let base = self
            .by_base
            .entry(item.base.clone())
            .or_insert(seed);
        base.count += 1;
        base.sum_dps += m.dps as f64;
        if m.ttk > 0.0 {
            base.weapon_count += 1;
            base.sum_ttk += m.ttk as f64;
            base.sum_kit_dps += m.kit_dps as f64;
            base.sum_kit_ttk += m.kit_ttk as f64;
            let acc = &mut base.ttk_by_rarity[item.rarity.index()];
            acc.weapon_count += 1;
            acc.sum_ttk += m.ttk as f64;
            for (i, &per_enemy_ttk) in m.ttk_per_enemy.iter().enumerate() {
                if i < base.ttk_by_enemy.len() && per_enemy_ttk > 0.0 {
                    base.ttk_by_enemy[i].weapon_count += 1;
                    base.ttk_by_enemy[i].sum_ttk += per_enemy_ttk as f64;
                }
            }
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
        enemies: &[Enemy],
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
            "{:<20} {:>8} {:>8} {:>10} {:>10} {:>10} {:>10}",
            "Base", "count", "pct", "avg_dps", "avg_ttk", "kit_dps", "kit_ttk"
        )?;
        for (base, sum) in &self.by_base {
            let pct = pct_of(sum.count as f64, self.drops as f64);
            let avg_dps = if sum.count > 0 {
                sum.sum_dps / sum.count as f64
            } else {
                0.0
            };
            let (ttk_cell, kit_dps_cell, kit_ttk_cell) = if sum.weapon_count > 0 {
                let wc = sum.weapon_count as f64;
                (
                    format!("{:>10.3}", sum.sum_ttk / wc),
                    format!("{:>10.2}", sum.sum_kit_dps / wc),
                    format!("{:>10.3}", sum.sum_kit_ttk / wc),
                )
            } else {
                (
                    format!("{:>10}", "—"),
                    format!("{:>10}", "—"),
                    format!("{:>10}", "—"),
                )
            };
            writeln!(
                out,
                "{:<20} {:>8} {:>7.2}% {:>10.2} {} {} {}",
                base, sum.count, pct, avg_dps, ttk_cell, kit_dps_cell, kit_ttk_cell
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

            // TTK by enemy — same drop-only data, sliced per-enemy from the
            // loaded roster. Column widths sized for "Swarm Rusher" (12) + space.
            if !enemies.is_empty() {
                write!(out, "{:<20}", "TTK by enemy")?;
                for e in enemies {
                    write!(out, " {:>13}", e.name)?;
                }
                writeln!(out)?;
                for (base, sum) in &self.by_base {
                    if sum.weapon_count == 0 {
                        continue;
                    }
                    write!(out, "{:<20}", base)?;
                    for acc in &sum.ttk_by_enemy {
                        if acc.weapon_count > 0 {
                            let avg = acc.sum_ttk / acc.weapon_count as f64;
                            write!(out, " {:>12.3}s", avg)?;
                        } else {
                            write!(out, " {:>13}", "—")?;
                        }
                    }
                    writeln!(out)?;
                }
                writeln!(out)?;
            }

            // Kills to reach each milestone level, per enemy. Invariant
            // relative to the run's RNG — pure function of xp_value × the
            // progression curve — but shown alongside TTK so the designer
            // sees "how long to kill" and "how many to level" together.
            let milestones: [u8; 4] = [5, 10, 20, 30];
            if !enemies.is_empty() && enemies.iter().any(|e| e.xp_value > 0) {
                write!(out, "{:<20}", "Kills to reach")?;
                for lvl in milestones {
                    write!(out, " {:>8}", format!("L{lvl}"))?;
                }
                writeln!(out)?;
                for e in enemies {
                    if e.xp_value == 0 {
                        continue;
                    }
                    write!(out, "{:<20}", e.name)?;
                    for lvl in milestones {
                        let xp_needed = progression::xp_to_reach(lvl);
                        let kills = xp_needed.div_ceil(e.xp_value as u64);
                        write!(out, " {:>8}", kills)?;
                    }
                    writeln!(out)?;
                }
                writeln!(out)?;
            }
        }

        writeln!(
            out,
            "{:<32} {:>4} {:>8} {:>10} {:>18} {:>10}",
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
                "{:<32} T{:<3} {:>8} {:>10.3} [{:>7.2}, {:>7.2}] {:>9.1}%",
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
