use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use clap::Parser;
use rand::{Rng, SeedableRng, rngs::StdRng};

use h2b_core::{ItemInstance, Rarity, item::RolledAffix, roll::roll_item};

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
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let affixes = h2b_content::load_affixes(&args.content_dir.join("affixes.ron"))?;
    let bases = h2b_content::load_bases(&args.content_dir.join("bases.ron"))?;

    let mut rng = StdRng::seed_from_u64(args.seed);
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());

    writeln!(out, "kill,rarity,base,ilvl,n_affixes,affixes")?;

    for kill in 0..args.kills {
        let rarity = roll_rarity(&mut rng);
        let item = roll_item(&mut rng, &bases, &affixes, args.monster_level, rarity)?;
        write_row(&mut out, kill, &item)?;
    }

    Ok(())
}

/// Placeholder rarity distribution. Real tuning happens once we eyeball the
/// first CSV — that's the whole point of the sim.
fn roll_rarity<R: Rng + ?Sized>(rng: &mut R) -> Rarity {
    let r = rng.gen_range(0..100);
    if r < 5 {
        Rarity::Rare
    } else if r < 30 {
        Rarity::Magic
    } else {
        Rarity::Normal
    }
}

fn write_row<W: Write>(out: &mut W, kill: u32, item: &ItemInstance) -> io::Result<()> {
    let n = item.prefixes.len() + item.suffixes.len();
    write!(
        out,
        "{kill},{rarity:?},{base},{ilvl},{n},",
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
