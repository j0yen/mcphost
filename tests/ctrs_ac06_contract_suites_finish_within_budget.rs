//! solo-suite: 770 one test re-runs every contract suite through pin-tests.sh (585-770 s on a hosted runner, the max of CI runs 39196582126, 39200390250, 39203293144)
//! PRD-mcphost-contract-suite-generated AC6 -- on a warm cargo target
//! `scripts/pin-tests.sh` finishes every contract suite within the hard budget
//! (900 s) and prints a per-binary timing line for each.
//!
//! The original 300 s wall-clock limit lives on as a target: above 300 s the
//! test prints a `::warning::` line and still passes. A shared GitHub runner's
//! wall clock is not a contract: on 10-17 the suites measured 312 s in run
//! 39069773737 (PR #407) and the core shard ran 666 s in run 39073233327 on
//! a523134c, which turned main red; on fast days they take 58-81 s.
//!
//! The target is warm by construction: the test binary that runs this already
//! built the crate, and `pin-tests.sh` builds the two contract binaries once
//! up front outside the timed section (a cold link is not what the budget
//! measures).

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

/// Hard budget: catches a hang or runaway, not a slow runner.
const BUDGET_SECS: u64 = 900;
/// Target: exceeding it only emits a `::warning::`.
// measured 611 s on CI run 39200390250 (PR #446, suite_contracts_core_01) + 25 %, rounded up to a minute
// Raising this again needs a new measured number in this comment (reviewer's check).
const TARGET_SECS: u64 = 780;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Drops the env cargo injects into a test process before running cargo (or
/// pin-tests.sh) in the running suite's target dir: same rule as
/// tests/common/mod.rs `strip_cargo_injected_env` (suite_core_04 has no
/// `mod common`). Without it `ring`'s build script sees CARGO_MANIFEST_DIR /
/// OUT_DIR "change", reruns, and the nested build relinks the binaries the
/// outer suite is running (the VFX flake class).
fn strip_cargo_injected_env(cmd: &mut Command) -> &mut Command {
    for (k, _) in std::env::vars_os() {
        let injected = k.to_str().is_some_and(|n| {
            ["CARGO_PKG_", "CARGO_MANIFEST_", "CARGO_BIN_EXE_"].iter().any(|p| n.starts_with(p))
                || ["CARGO_BIN_NAME", "CARGO_CRATE_NAME", "CARGO_PRIMARY_PACKAGE", "CARGO_TARGET_TMPDIR", "OUT_DIR"].contains(&n)
        });
        if injected {
            cmd.env_remove(k);
        }
    }
    cmd
}

#[test]
fn ctrs_ac06_pin_tests_runs_all_contract_suites_within_budget_and_prints_timings() {
    let list = strip_cargo_injected_env(&mut Command::new(root().join("scripts/pin-tests.sh")))
        .arg("--list")
        .current_dir(root())
        .output()
        .unwrap();
    let bins: Vec<String> = String::from_utf8_lossy(&list.stdout).lines().map(str::to_string).collect();
    assert!(!bins.is_empty());

    // warm the target (compile only), untimed
    for b in &bins {
        let st = strip_cargo_injected_env(&mut Command::new(env!("CARGO")))
            .args(["test", "-q", "--no-run", "--test", b])
            .current_dir(root())
            .status()
            .unwrap();
        assert!(st.success(), "could not build {b}");
    }

    let start = Instant::now();
    let out = strip_cargo_injected_env(&mut Command::new(root().join("scripts/pin-tests.sh")))
        .current_dir(root())
        .output()
        .unwrap();
    let elapsed = start.elapsed().as_secs();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "pin-tests.sh failed:\n{stdout}");
    if elapsed > TARGET_SECS {
        println!("::warning::contract suites took {elapsed}s (target {TARGET_SECS}s)");
    }
    assert!(elapsed <= BUDGET_SECS, "contract suites took {elapsed}s, budget {BUDGET_SECS}s:\n{stdout}");
    for b in &bins {
        let prefix = format!("pin-tests: pass {b} (");
        let line = stdout.lines().find(|l| l.starts_with(&prefix)).unwrap_or_else(|| panic!("no timing line for {b}:\n{stdout}"));
        assert!(line.ends_with("s)"), "{line}");
    }
    assert!(stdout.contains("pin-tests: ok ("), "{stdout}");
}
