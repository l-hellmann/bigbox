//! End-to-end snapshot tests. Invokes the `bb-sim` binary, captures stdout,
//! and locks it via `insta`. Any drift in roll mechanics, aggregation, or
//! starter content trips the snapshot diff, forcing conscious review.
//!
//! To update snapshots after an intentional change:
//!   cargo install cargo-insta   # one-time
//!   cargo insta review          # inspect + accept per-snapshot
//! Or set `INSTA_UPDATE=always` and re-run `cargo test`.

use std::process::Command;

fn run_sim(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_bb-sim"))
        .args(args)
        .arg("--content-dir")
        .arg("../content/data")
        .output()
        .expect("failed to spawn bb-sim");

    assert!(
        output.status.success(),
        "bb-sim exited non-zero (status {:?}). stderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );

    String::from_utf8(output.stdout).expect("bb-sim stdout not valid utf-8")
}

#[test]
fn summary_seed42_ilvl60_2k_kills() {
    let out = run_sim(&[
        "--monster-level", "60",
        "--kills", "2000",
        "--seed", "42",
        "--summary",
    ]);
    insta::assert_snapshot!(out);
}

#[test]
fn summary_seed42_ilvl20_2k_kills() {
    // ilvl 20 gates everything above T3 — exercises the ilvl filter,
    // including Legendary's tier-floor interaction (floor T2 but no T2
    // affixes eligible at this ilvl).
    let out = run_sim(&[
        "--monster-level", "20",
        "--kills", "2000",
        "--seed", "42",
        "--summary",
    ]);
    insta::assert_snapshot!(out);
}

#[test]
fn csv_seed7_ilvl40_first_10_kills() {
    // Small CSV sample exercises the row-format path and dps_estimate column.
    let out = run_sim(&[
        "--monster-level", "40",
        "--kills", "10",
        "--seed", "7",
    ]);
    insta::assert_snapshot!(out);
}
