//! Snapshot tests for the BSP map generator. Locks the ASCII output of
//! known seeds so accidental changes to the algorithm or RNG threading
//! show up immediately as a diff.
//!
//! Update with `cargo insta review` or `INSTA_UPDATE=always cargo test`.

use h2b_procgen::{MapParams, generate_bsp};

#[test]
fn bsp_seed_42_default_params() {
    let map = generate_bsp(&MapParams {
        seed: 42,
        ..Default::default()
    });
    insta::assert_snapshot!(map.render_ascii());
}

#[test]
fn bsp_seed_7_small_map() {
    // Smaller dimensions exercise the "can't split further" leaf path.
    let map = generate_bsp(&MapParams {
        seed: 7,
        width: 30,
        height: 20,
        min_room_size: 4,
        max_room_size: 8,
        max_depth: 4,
        ..Default::default()
    });
    insta::assert_snapshot!(map.render_ascii());
}
