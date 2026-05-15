use std::collections::HashMap;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use clap::Parser;
use rand::{Rng, SeedableRng, rngs::StdRng};

use h2b_core::{
    BaseItem, Combatant, ItemInstance, Rarity, StatId, Weapon, aggregate_item, dps_against,
    item::RolledAffix, roll::roll_item,
};

mod summary;
use summary::Summary;

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
    let base_index: HashMap<&str, &BaseItem> =
        bases.iter().map(|b| (b.id.as_str(), b)).collect();

    let mut rng = StdRng::seed_from_u64(args.seed);
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());

    if args.summary {
        let mut sum = Summary::new();
        for _ in 0..args.kills {
            let rarity = roll_rarity(&mut rng);
            let item = roll_item(&mut rng, &bases, &affixes, args.monster_level, rarity)?;
            let stats = aggregate_item(&item, base_index[item.base.as_str()], &affixes);
            let dps = weapon_dps(&stats);
            sum.record(&item, dps);
        }
        sum.print(&mut out, &affixes, args.seed, args.monster_level)?;
    } else {
        writeln!(out, "kill,rarity,base,ilvl,n_affixes,dps_estimate,affixes")?;
        for kill in 0..args.kills {
            let rarity = roll_rarity(&mut rng);
            let item = roll_item(&mut rng, &bases, &affixes, args.monster_level, rarity)?;
            let stats = aggregate_item(&item, base_index[item.base.as_str()], &affixes);
            let dps = weapon_dps(&stats);
            write_row(&mut out, kill, &item, dps)?;
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

/// Weapon DPS against a naked baseline target (no armor, no evasion).
/// Crit and damage-type aggregation live in `core::combat` — this is just the
/// sim's normalized comparison metric. Armor items naturally return 0
/// (no fire rate → no DPS).
fn weapon_dps(stats: &HashMap<StatId, f32>) -> f32 {
    let weapon = Weapon::from_stats(stats);
    dps_against(&weapon, &Combatant::dummy(1.0))
}

fn write_row<W: Write>(out: &mut W, kill: u32, item: &ItemInstance, dps: f32) -> io::Result<()> {
    let n = item.prefixes.len() + item.suffixes.len();
    write!(
        out,
        "{kill},{rarity:?},{base},{ilvl},{n},{dps:.2},",
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
