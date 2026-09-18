//! PRD-mcphost-proof-lane-loop-config, AC6 (P1 requirement 4) — a malformed
//! lane fails the coverage proof, naming the lane id, instead of surfacing
//! later as a vti-plan `GlobSetBuilder` error with no PRD context.
//!
//! Both cases run against synthetic lanes so the proof never depends on the
//! shipped `agent/proof-lanes.toml` staying broken (or staying fixed) to
//! show the check works.

use crate::lanecov;

use lanecov::{validate_lane_shapes, Lane};

/// AC6: a lane with an empty `required_commands` must fail validation,
/// naming the lane id.
#[test]
fn lanecov_ac06_empty_required_commands_fails_naming_the_lane() {
    let lanes = vec![Lane {
        id: "broken-lane".to_string(),
        description: "synthetic".to_string(),
        globs: vec!["synthetic/**".to_string()],
        required_commands: vec![],
        note: None,
    }];
    let err =
        validate_lane_shapes(&lanes).expect_err("empty required_commands must fail validation");
    assert!(err.contains("broken-lane"), "error must name the lane id: {err}");
}

/// P1 requirement 4's other half: a lane whose glob does not compile must
/// fail validation, naming the lane id.
#[test]
fn lanecov_ac06_uncompilable_glob_fails_naming_the_lane() {
    let lanes = vec![Lane {
        id: "unbalanced-lane".to_string(),
        description: "synthetic".to_string(),
        // An unterminated character class -- globset rejects this at
        // `Glob::new` time.
        globs: vec!["src/[abc".to_string()],
        required_commands: vec!["cargo test --workspace".to_string()],
        note: None,
    }];
    let err = validate_lane_shapes(&lanes).expect_err("an invalid glob must fail validation");
    assert!(
        err.contains("unbalanced-lane"),
        "error must name the lane id: {err}"
    );
}

/// The positive control for both cases above: a well-formed lane validates.
/// Without this, a `validate_lane_shapes` that rejected everything would
/// still pass the two negative tests.
#[test]
fn lanecov_ac06_well_formed_lane_validates() {
    let lanes = vec![Lane {
        id: "healthy-lane".to_string(),
        description: "synthetic".to_string(),
        globs: vec!["src/**".to_string()],
        required_commands: vec!["cargo test --workspace".to_string()],
        note: None,
    }];
    let sets = validate_lane_shapes(&lanes).expect("a well-formed lane must validate");
    assert_eq!(sets.len(), 1, "one lane in, one globset out");
}
