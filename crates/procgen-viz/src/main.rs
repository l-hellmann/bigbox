//! Procgen visualizer. Generates a BSP map for a seed, optionally overlays
//! the flow field and weighted spawn picks, prints to stdout.
//!
//! Examples:
//!   h2b-procgen-viz --seed 42
//!   h2b-procgen-viz --seed 7 --width 50 --height 20 --color
//!   h2b-procgen-viz --seed 42 --enemies 12 --flow --color

use std::io::{self, BufWriter, Write};

use clap::Parser;
use rand::{SeedableRng, rngs::StdRng};

use h2b_procgen::{FlowField, MapParams, Tile, UNREACHABLE, generate_bsp, pick_spawn_points};

#[derive(Parser, Debug)]
#[command(name = "h2b-procgen-viz", about = "Visualize a procgen map (ASCII).")]
struct Args {
    #[arg(long, default_value_t = 0)]
    seed: u64,
    #[arg(long, default_value_t = 80)]
    width: u32,
    #[arg(long, default_value_t = 40)]
    height: u32,
    #[arg(long = "min-room", default_value_t = 5)]
    min_room: u32,
    #[arg(long = "max-room", default_value_t = 12)]
    max_room: u32,
    #[arg(long = "max-depth", default_value_t = 5)]
    max_depth: u8,
    /// Place N enemy spawn markers (`Z`), weighted by flow-field distance.
    #[arg(long, default_value_t = 0)]
    enemies: u32,
    /// Minimum tile-distance an enemy spawn must be from the player spawn.
    #[arg(long = "min-dist", default_value_t = 8)]
    min_dist: u32,
    /// Replace each floor tile with `distance_to_spawn % 10` so the gradient
    /// is visible at a glance.
    #[arg(long)]
    flow: bool,
    /// Emit ANSI-colored output. Off by default so piping into a file is
    /// clean; on for interactive use.
    #[arg(long)]
    color: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let map = generate_bsp(&MapParams {
        width: args.width,
        height: args.height,
        min_room_size: args.min_room,
        max_room_size: args.max_room,
        max_depth: args.max_depth,
        seed: args.seed,
    });

    let field = if args.flow || args.enemies > 0 {
        Some(FlowField::compute(&map, map.player_spawn))
    } else {
        None
    };

    let spawns: Vec<(u32, u32)> = if args.enemies > 0 {
        let mut rng = StdRng::seed_from_u64(args.seed.wrapping_add(1));
        pick_spawn_points(
            &mut rng,
            field.as_ref().unwrap(),
            args.enemies as usize,
            args.min_dist,
        )
    } else {
        Vec::new()
    };

    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());

    writeln!(
        out,
        "seed: {}  size: {}x{}  rooms: {}  spawn: {:?}  enemy_spawns: {}",
        args.seed,
        map.width,
        map.height,
        map.rooms.len(),
        map.player_spawn,
        spawns.len(),
    )?;

    render(&mut out, &map, field.as_ref(), &spawns, &args)?;

    Ok(())
}

fn render<W: Write>(
    out: &mut W,
    map: &h2b_procgen::Map,
    field: Option<&FlowField>,
    spawns: &[(u32, u32)],
    args: &Args,
) -> io::Result<()> {
    let spawn_set: std::collections::HashSet<(u32, u32)> = spawns.iter().copied().collect();

    for y in 0..map.height {
        for x in 0..map.width {
            let here = (x, y);

            if here == map.player_spawn {
                paint(out, '@', Color::Player, args.color)?;
                continue;
            }
            if spawn_set.contains(&here) {
                paint(out, 'Z', Color::Enemy, args.color)?;
                continue;
            }

            match map.tile_at(x, y) {
                Tile::Wall => paint(out, '#', Color::Wall, args.color)?,
                Tile::Floor => {
                    if args.flow {
                        let d = field.expect("flow field built when --flow").distance_at(x, y);
                        if d == UNREACHABLE {
                            paint(out, '?', Color::Wall, args.color)?;
                        } else {
                            let ch = char::from_digit((d % 10) as u32, 10).unwrap_or('.');
                            paint(out, ch, Color::Floor, args.color)?;
                        }
                    } else {
                        paint(out, '.', Color::Floor, args.color)?;
                    }
                }
            }
        }
        writeln!(out)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Color {
    Wall,
    Floor,
    Player,
    Enemy,
}

fn paint<W: Write>(out: &mut W, ch: char, c: Color, on: bool) -> io::Result<()> {
    if !on {
        return write!(out, "{ch}");
    }
    let code = match c {
        Color::Wall => "\x1b[90m",     // bright black / gray
        Color::Floor => "\x1b[2;37m",  // dim white
        Color::Player => "\x1b[1;92m", // bright bold green
        Color::Enemy => "\x1b[1;91m",  // bright bold red
    };
    write!(out, "{code}{ch}\x1b[0m")
}
