use std::collections::HashMap;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use clap::Parser;
use rand::{Rng, SeedableRng, rngs::StdRng};

use h2b_core::{
    Attachment, BaseItem, Combatant, ItemInstance, Rarity, StatId, Weapon, aggregate_item,
    dps_against, item::RolledAffix, roll::roll_item, time_to_kill,
};

mod summary;
use summary::{DropMetrics, Summary};

#[derive(Parser, Debug)]
#[command(name = "h2b-sim", about = "head2box loot drop simulator")]
struct Args {
    #[arg(long)]
    monster_level: u32,
    #[arg(long)]
    kills: u32,
    #[arg(long)]
    seed: u64,
    #[arg(long, default_value = "crates/content/data")]
    content_dir: PathBuf,
    /// Print distribution tables instead of per-row CSV.
    #[arg(long)]
    summary: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let affixes = h2b_content::load_affixes(&args.content_dir.join("affixes.ron"))?;
    let bases = h2b_content::load_bases(&args.content_dir.join("bases.ron"))?;
    let attachments = h2b_content::load_attachments(&args.content_dir.join("attachments.ron"))?;
    let base_index: HashMap<&str, &BaseItem> =
        bases.iter().map(|b| (b.id.as_str(), b)).collect();
    let optimal_loadouts: HashMap<&str, Vec<String>> = bases
        .iter()
        .map(|b| (b.id.as_str(), optimal_loadout(b, &attachments)))
        .collect();

    let mut rng = StdRng::seed_from_u64(args.seed);
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());

    if args.summary {
        let mut sum = Summary::new();
        for _ in 0..args.kills {
            let rarity = roll_rarity(&mut rng);
            let item = roll_item(&mut rng, &bases, &affixes, args.monster_level, rarity)?;
            let base = base_index[item.base.as_str()];
            let loadout = &optimal_loadouts[item.base.as_str()];
            let metrics = measure(&item, base, &affixes, &attachments, loadout);
            sum.record(&item, &metrics);
        }
        sum.print(&mut out, &affixes, args.seed, args.monster_level)?;
    } else {
        writeln!(
            out,
            "kill,rarity,base,ilvl,n_affixes,dps_estimate,ttk_estimate,affixes"
        )?;
        for kill in 0..args.kills {
            let rarity = roll_rarity(&mut rng);
            let item = roll_item(&mut rng, &bases, &affixes, args.monster_level, rarity)?;
            let stats = aggregate_item(&item, base_index[item.base.as_str()], &affixes, &[]);
            let m = naked_metrics(&stats);
            write_row(&mut out, kill, &item, m.0, m.1)?;
        }
    }

    Ok(())
}

/// Starter rarity distribution (per 1000): Basic 600, Common 280, Rare 90,
/// Epic 25, Legendary 5. Tunable once we eyeball the summary.
fn roll_rarity<R: Rng + ?Sized>(rng: &mut R) -> Rarity {
    let r = rng.gen_range(0..1000);
    if r < 5 {
        Rarity::Legendary
    } else if r < 30 {
        Rarity::Epic
    } else if r < 120 {
        Rarity::Rare
    } else if r < 400 {
        Rarity::Common
    } else {
        Rarity::Basic
    }
}

/// Benchmark target for TTK measurement. Modest armor exercises mitigation
/// math without disproportionately punishing any archetype.
fn benchmark_enemy() -> Combatant {
    Combatant {
        max_life: 200.0,
        current_life: 200.0,
        armor: 30.0,
        evasion: 0.0,
    }
}

/// Pre-compute the attachment combo that maxes naked-weapon DPS for this
/// base. Enumerated by Cartesian product over compatible attachments per
/// slot — the combo space is tiny (≤ ~12 per weapon in current content),
/// so brute force is fine. Slots with no compatible attachment are skipped.
fn optimal_loadout(base: &BaseItem, attachments: &[Attachment]) -> Vec<String> {
    let by_slot: Vec<Vec<&Attachment>> = base
        .attachment_slots
        .iter()
        .map(|slot| {
            attachments
                .iter()
                .filter(|a| {
                    &a.slot_type == slot
                        && a.allowed_categories
                            .iter()
                            .any(|c| c == &base.category)
                })
                .collect()
        })
        .filter(|v: &Vec<&Attachment>| !v.is_empty())
        .collect();

    if by_slot.is_empty() {
        return Vec::new();
    }

    let mut best_dps = -1.0_f32;
    let mut best: Vec<String> = Vec::new();
    let mut indices = vec![0usize; by_slot.len()];
    loop {
        let combo: Vec<String> = indices
            .iter()
            .zip(by_slot.iter())
            .map(|(&i, slot)| slot[i].id.clone())
            .collect();

        // Build a tier-0 zero-affix instance with this combo, measure DPS
        // against the naked dummy. We're comparing combos against each other,
        // so the absolute number doesn't matter — only the ranking.
        let item = ItemInstance {
            base: base.id.clone(),
            ilvl: 60,
            rarity: Rarity::Basic,
            seed: 0,
            prefixes: vec![],
            suffixes: vec![],
            upgrade_tier: 0,
            attached: combo.clone(),
        };
        let stats = aggregate_item(&item, base, &[], attachments);
        let weapon = Weapon::from_stats(&stats);
        let dps = dps_against(&weapon, &Combatant::dummy(1.0));
        if dps > best_dps {
            best_dps = dps;
            best = combo;
        }

        // Odometer increment.
        let mut bumped = false;
        for i in (0..indices.len()).rev() {
            indices[i] += 1;
            if indices[i] < by_slot[i].len() {
                bumped = true;
                break;
            }
            indices[i] = 0;
        }
        if !bumped {
            break;
        }
    }
    best
}

fn measure(
    item: &ItemInstance,
    base: &BaseItem,
    affixes: &[h2b_core::Affix],
    attachments: &[Attachment],
    optimal_combo: &[String],
) -> DropMetrics {
    let naked = aggregate_item(item, base, affixes, &[]);
    let (dps, ttk) = naked_metrics(&naked);

    // Kitted = same drop + optimal attachments slotted.
    let mut kitted_item = item.clone();
    kitted_item.attached = optimal_combo.to_vec();
    let kitted = aggregate_item(&kitted_item, base, affixes, attachments);
    let (kit_dps, kit_ttk) = naked_metrics(&kitted);

    DropMetrics {
        dps,
        ttk,
        kit_dps,
        kit_ttk,
    }
}

/// Compute (dps, ttk) for a stats map. "Naked" here just means "as-given",
/// not literally without attachments — the caller decides what's in the map.
fn naked_metrics(stats: &HashMap<StatId, f32>) -> (f32, f32) {
    let weapon = Weapon::from_stats(stats);
    let dps = dps_against(&weapon, &Combatant::dummy(1.0));
    let ttk = if dps > 0.0 {
        time_to_kill(&weapon, &benchmark_enemy()).unwrap_or(0.0)
    } else {
        0.0
    };
    (dps, ttk)
}

fn write_row<W: Write>(
    out: &mut W,
    kill: u32,
    item: &ItemInstance,
    dps: f32,
    ttk: f32,
) -> io::Result<()> {
    let n = item.prefixes.len() + item.suffixes.len();
    write!(
        out,
        "{kill},{rarity:?},{base},{ilvl},{n},{dps:.2},{ttk:.3},",
        rarity = item.rarity,
        base = item.base,
        ilvl = item.ilvl,
    )?;
    let mut first = true;
    for a in item.prefixes.iter().chain(item.suffixes.iter()) {
        if !first {
            write!(out, "|")?;
        }
        first = false;
        write_affix(out, a)?;
    }
    writeln!(out)
}

fn write_affix<W: Write>(out: &mut W, a: &RolledAffix) -> io::Result<()> {
    write!(out, "T{}_{}", a.tier, a.affix_id)?;
    if !a.rolls.is_empty() {
        write!(out, ":")?;
        let mut first = true;
        for r in &a.rolls {
            if !first {
                write!(out, "/")?;
            }
            first = false;
            write!(out, "{r:.2}")?;
        }
    }
    Ok(())
}
