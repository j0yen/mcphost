//! PRD-mcphost-test-suite-consolidation
//! AC5 (P0) -- Given a clean `target/`, When `cargo nextest run` completes,
//! Then `du -s target/debug/deps` is under 5GB and the receipt records the
//! size.
//!
//! `#[ignore]`d like `ac11_load_smoke.rs`: the number depends on how much
//! else lives in `$CARGO_TARGET_DIR` (a shared, long-lived dev-box cache
//! accumulates unrelated crates/binaries across a build box's whole
//! history), so it is only meaningful measured against a genuinely fresh
//! target dir, which a routine `cargo test` run must not force on every
//! contributor. Run it directly with a clean, ISOLATED target dir:
//! `CARGO_TARGET_DIR=$(mktemp -d) cargo test --release --test suite_core_04 suite_ac5_disk_size_budget:: -- --ignored --nocapture`
//! (any of the 6 suite binaries plus `--lib`/`--bin` would do; this uses
//! `--release` only because that's the shape `ac11_load_smoke.rs` already
//! established for a manually-run receipt test in this repo, not because
//! the P0 requirement itself specifies a profile).
//!
//! Measured on the build box (RedBaron) at the migration commit, isolated
//! `CARGO_TARGET_DIR`, clean `cargo nextest run` (`debug` profile, the
//! profile nextest actually links): **`target/debug/deps` = 3.75 GB**, well
//! under the 5 GB budget, against a directly-measured 78 GB for the same
//! dependency set at the pre-change (289-binary) layout.
//! See docs/benchmarks/suite-consolidation-disk-size.txt (copy-claims.sh
//! checks that file's presence, matching the ac11-load-smoke.txt
//! convention) -- re-run and update that file's number together with this
//! comment if the area table or dependency set changes materially.

use std::path::PathBuf;

const BUDGET_BYTES: u64 = 5 * 1024 * 1024 * 1024; // 5 GB, P0 requirement.

fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            total += dir_size(&p);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

#[test]
#[ignore = "hardware/cache-dependent; run with an isolated CARGO_TARGET_DIR after a clean `cargo nextest run` -- see doc comment"]
fn target_debug_deps_under_five_gib() {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
    let deps = target_dir.join("debug").join("deps");
    let size = dir_size(&deps);
    println!(
        "target-debug-deps-bytes: {size} ({:.2} GB) budget: {BUDGET_BYTES} ({:.2} GB)",
        size as f64 / 1e9,
        BUDGET_BYTES as f64 / 1e9,
    );
    assert!(
        size > 0,
        "{} reported 0 bytes -- probably not a real, built target dir (did \
         you run `cargo nextest run` first)?",
        deps.display()
    );
    assert!(
        size < BUDGET_BYTES,
        "{} is {size} bytes ({:.2} GB), over the 5 GB P0 budget",
        deps.display(),
        size as f64 / 1e9,
    );
}
