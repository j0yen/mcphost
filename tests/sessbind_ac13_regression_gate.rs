//! PRD-mcphost-session-bound-tenant-after-signup
//! AC13 (P0) — Given `cargo test` at the landing commit, When it runs,
//! Then every test that passed at v0.60.35 still passes (the `tkparam` and
//! `oauthrs` suites unchanged) and the `sessbind_*` tests for AC 1-12
//! pass, with the gate receipt showing block=0.
//!
//! `cargo test` itself is the actual proof of "every test still passes" --
//! this file adds narrower, durable checks that don't depend on any
//! particular commit sha (this repo's own convention: a test that asserts
//! "no changes since sha X" self-invalidates the moment landing adds a
//! commit): the version bump landed, the `tkparam`/`oauthrs` suites still
//! assert the literal codes they've always asserted, and the receipt --
//! same "regression lock on a checked-in receipt" pattern
//! `tests/tkparam_ac11_full_suite_green_at_landing.rs` and
//! `tests/checkcompat_race_ac07_suite_green_and_clippy_clean.rs` use --
//! records a real `cargo test --workspace` / `cargo clippy` run.
//!
//! `block=0` means no AC of this PRD is blocked: AC1-AC13 are all paired
//! with a hermetic test that runs on every `cargo test` (see
//! `agent/test-map.json`). The receipt names each one with its test file, so
//! "block=0" is readable as a list rather than taken on trust. (AC14 is the
//! operator's own post-deploy explore-run check against production and is
//! outside this build entirely -- it is not counted here either way.)

use std::fs;
use std::path::Path;

/// Parses the `[package]` `version = "X.Y.Z"` line (the only bare
/// `^version = ` line in this Cargo.toml -- dependency versions are always
/// written `crate = { version = "x", ... }`, never on their own line).
fn package_minor(cargo_toml: &str) -> (u64, u64) {
    let line = cargo_toml
        .lines()
        .find(|l| l.starts_with("version = "))
        .unwrap_or_else(|| panic!("no top-level version line in Cargo.toml: {cargo_toml}"));
    let raw = line
        .trim_start_matches("version = \"")
        .trim_end_matches('"');
    let mut parts = raw.split('.');
    let major: u64 = parts.next().unwrap().parse().expect("major");
    let minor: u64 = parts.next().unwrap().parse().expect("minor");
    (major, minor)
}

#[test]
fn version_bumped_minor() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cargo_toml = fs::read_to_string(manifest_dir.join("Cargo.toml")).expect("read Cargo.toml");
    // PRD's own version_bump rule is minor (0.60.35 -> 0.61.0). Checked as
    // a floor ((major, minor) >= (0, 61)), not an exact string match: an
    // exact match self-invalidates the moment ANY later PRD bumps the
    // version again (landing adds a release commit on every subsequent
    // PR, same class of bug this repo's suite-gate tests already avoid
    // for commit shas -- see module doc above). A floor never invalidates
    // again since this repo's version only moves forward.
    assert!(
        package_minor(&cargo_toml) >= (0, 61),
        "PRD's own version_bump rule is minor (0.60.35 -> 0.61.0), so the package version must be \
         at least 0.61 forever after this lands: {cargo_toml}"
    );
}

#[test]
fn tkparam_and_oauthrs_suites_still_assert_their_documented_codes() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    let tkparam = fs::read_to_string(manifest_dir.join("tests/tkparam_ac03_five_tools_missing_tenant_key.rs"))
        .expect("tkparam_ac03 must still exist");
    assert!(
        tkparam.contains("-32602") && tkparam.contains("tenant_key_missing"),
        "tkparam_ac03 must still assert -32602/tenant_key_missing for a key-less host.* call"
    );

    let oauthrs = fs::read_to_string(manifest_dir.join("tests/oauthrs_ac10_key_based_flow_unchanged.rs"))
        .expect("oauthrs_ac10 must still exist");
    assert!(
        oauthrs.contains("a key-authenticated caller has no OAuth subject"),
        "oauthrs_ac10 must still assert the key-based flow's own unchanged whoami shape"
    );
}

#[test]
fn suite_gate_receipt_records_zero_failures_zero_clippy_warnings_and_every_deferral() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/benchmarks/sessbind-suite-gate.txt");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

    assert!(
        text.contains("0 failed"),
        "receipt must record a zero-failure cargo test --workspace run, got: {text}"
    );
    assert!(
        text.contains("clippy_warning_count: 0"),
        "receipt must record zero clippy warnings, got: {text}"
    );
    assert!(
        text.contains("block=0"),
        "receipt must record block=0 -- no AC of this PRD is blocked, got: {text}"
    );
    for ac in [
        "AC1", "AC2", "AC3", "AC4", "AC5", "AC6", "AC7", "AC8", "AC9", "AC10", "AC11", "AC12",
        "AC13",
    ] {
        assert!(
            text.contains(ac),
            "receipt must name every AC explicitly with the test that pairs it, missing \
             {ac}: {text}"
        );
    }
}
