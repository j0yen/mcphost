//! PRD-mcphost-proof-lane-loop-config, AC3 — an unrouted tracked path is
//! NAMED in the failure, not merely counted.
//!
//! Exercised against a synthetic lane set + path list (the reusable
//! `find_unrouted` core AC1 itself calls) rather than a real scratch git
//! clone: the two scenarios cannot disagree by construction, and a scratch
//! clone would make the proof depend on network/disk state the gate cannot
//! reproduce.

#[path = "support/lanecov.rs"]
mod lanecov;

use lanecov::{find_unrouted, validate_lane_shapes, Lane};

/// AC3: given a tracked path (`zz-unrouted/x.txt`) that matches no lane,
/// the report names exactly that path.
#[test]
fn lanecov_ac03_unrouted_path_is_named() {
    let lanes = vec![Lane {
        id: "src-only".to_string(),
        description: "synthetic".to_string(),
        globs: vec!["src/**".to_string()],
        required_commands: vec!["cargo test --workspace".to_string()],
        note: None,
    }];
    let lane_sets = validate_lane_shapes(&lanes).expect("synthetic lane must validate");
    let paths = vec!["src/main.rs".to_string(), "zz-unrouted/x.txt".to_string()];
    let unrouted = find_unrouted(&lane_sets, &paths, &[]);
    assert_eq!(
        unrouted,
        vec!["zz-unrouted/x.txt"],
        "the synthetic unrouted path must be named and nothing else"
    );
}

/// AC3's negative half: once a lane covers the path, it disappears from the
/// report -- so the test above is proving routing, not just echoing its
/// input list back.
#[test]
fn lanecov_ac03_routed_path_is_not_named() {
    let lanes = vec![Lane {
        id: "catch-all".to_string(),
        description: "synthetic".to_string(),
        globs: vec!["src/**".to_string(), "zz-unrouted/**".to_string()],
        required_commands: vec!["cargo test --workspace".to_string()],
        note: None,
    }];
    let lane_sets = validate_lane_shapes(&lanes).expect("synthetic lane must validate");
    let paths = vec!["src/main.rs".to_string(), "zz-unrouted/x.txt".to_string()];
    let unrouted = find_unrouted(&lane_sets, &paths, &[]);
    assert!(
        unrouted.is_empty(),
        "a path covered by a lane glob must not be reported unrouted: {unrouted:?}"
    );
}
