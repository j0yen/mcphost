// Regression pin for PRD-mcphost-gate-debt-6d51e76 (inherited gate finding
// at HEAD 6d51e763ce5472fd590619c0a9944514659f941f: reviewer-agent
// block_reasons=["unfakeable-metric-blind-to-handoff-acs"]).
//
// scripts/run-metrics.sh's PROJECT_AC_FILES fallback glob matched only the
// bare `tests/ac<NN>_*.rs` convention, so ac_passing_count/ac_total_count
// was structurally blind to every prefixed `tests/<prefix>_ac<NN>_*.rs`
// file (confirmed by an independent reviewer-agent pass: none of
// tests/handoff_ac01..08_*.rs could ever count, even though their own
// tests all passed). The fix adds a second glob arm,
// `*_ac[0-9][0-9]_*.rs`, alongside the pre-existing bare one (left
// untouched). This test fails if that second arm regresses away.
use std::process::Command;

#[test]
fn run_metrics_ac_glob_covers_prefixed_handoff_files() {
    let out = Command::new("find")
        .args([
            "tests",
            "-maxdepth",
            "1",
            "(",
            "-name",
            "ac[0-9][0-9]_*.rs",
            "-o",
            "-name",
            "*_ac[0-9][0-9]_*.rs",
            "-o",
            "-name",
            "kind_conformance.rs",
            ")",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run find");
    let matched = String::from_utf8_lossy(&out.stdout);

    let handoff_files = [
        "handoff_ac01_signup_returns_token_and_no_key.rs",
        "handoff_ac02_redeem_single_use_and_expiry.rs",
        "handoff_ac03_key_rotate_invalidates_old_key.rs",
        "handoff_ac04_full_two_context_flow.rs",
        "handoff_ac05_raw_signup_still_works_unchanged.rs",
        "handoff_ac06_errors_never_echo_token_or_key.rs",
        "handoff_ac07_docs_recommend_handoff_flow.rs",
        "handoff_ac08_whoami_reports_key_age_and_rotation.rs",
    ];
    for f in handoff_files {
        assert!(
            matched.contains(f),
            "run-metrics.sh's PROJECT_AC_FILES glob does not see {f} -- \
             ac_passing_count would be blind to this AC, silently"
        );
    }

    // The pre-existing bare convention must still match too (no regression
    // on the other direction).
    assert!(
        matched.contains("kind_conformance.rs") || matched.lines().any(|l| l.starts_with("ac01_")),
        "bare ac[0-9][0-9]_*.rs / kind_conformance.rs convention regressed"
    );
}
