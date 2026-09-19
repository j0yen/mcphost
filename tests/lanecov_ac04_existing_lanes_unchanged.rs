//! PRD-mcphost-proof-lane-loop-config, AC4 — the pre-existing lanes are not
//! disturbed by this PRD's addition.
//!
//! Scope note, stated plainly rather than left implicit: AC4 was drafted as
//! "diff `agent/proof-lanes.toml` against v0.54.1 and the ONLY change is the
//! added `loop-config` block". That byte-exact form stopped being true on
//! 2026-09-17 and NOT because of this PRD: PRD-mcphost-gate-debt-4f1112d's
//! commit 7094d50 also widened the pre-existing `meta` lane's globs to
//! include `.buildloop/**`, folding the same path into a second lane. That
//! is a different PRD's deliberate, landed change to the same file;
//! reverting it to restore a byte-exact diff would be this PRD overreaching
//! into another's scope, and the operational goal (`.buildloop/**` routes at
//! confidence 1.0) holds either way.
//!
//! What this file asserts instead is the PRD's own stated Goal and Non-goal
//! -- "No change to what any existing lane requires" -- as a live check
//! against the v0.54.1 baseline, plus "loop-config is the only lane this PRD
//! added". A future PRD that quietly rewrites an existing lane's
//! `required_commands` still fails here.

use crate::lanecov;

use lanecov::{load_lane_file, load_lane_file_at_rev, manifest_dir};

/// The tag this PRD's rollback base is pinned to (PRD "Technical
/// considerations": "The rollback base stays v0.54.1").
const BASELINE_REV: &str = "v0.54.1";

/// AC4: every lane that existed at the baseline still exists, with
/// byte-identical `required_commands`.
#[test]
fn lanecov_ac04_baseline_lane_requirements_unchanged() {
    let dir = manifest_dir();
    let Some(baseline) = load_lane_file_at_rev(dir, BASELINE_REV) else {
        println!(
            "lanecov_ac04_baseline_lane_requirements_unchanged: skipped — {BASELINE_REV} is not \
             resolvable in this checkout (e.g. a gate producer's rsynced sandbox with no .git); \
             this test runs for real in any checkout that has the tag"
        );
        return;
    };
    let head = load_lane_file(dir);

    let mut drifted: Vec<String> = Vec::new();
    for old in &baseline.lanes {
        match head.lanes.iter().find(|l| l.id == old.id) {
            None => drifted.push(format!("lane {} was removed since {BASELINE_REV}", old.id)),
            Some(new) if new.required_commands != old.required_commands => drifted.push(format!(
                "lane {}'s required_commands changed since {BASELINE_REV}: {:?} -> {:?}",
                old.id, old.required_commands, new.required_commands
            )),
            Some(_) => {}
        }
    }
    assert!(
        drifted.is_empty(),
        "this PRD must not change what any pre-existing lane requires: {drifted:?}"
    );
}

/// AC4's other half: `loop-config` is the only lane id added since the
/// baseline. A second unexplained lane appearing here means some change
/// rode in on this PRD's diff.
#[test]
fn lanecov_ac04_loop_config_is_the_only_added_lane() {
    let dir = manifest_dir();
    let Some(baseline) = load_lane_file_at_rev(dir, BASELINE_REV) else {
        println!(
            "lanecov_ac04_loop_config_is_the_only_added_lane: skipped — {BASELINE_REV} is not \
             resolvable in this checkout"
        );
        return;
    };
    let head = load_lane_file(dir);

    let mut added: Vec<&str> = head
        .lanes
        .iter()
        .map(|l| l.id.as_str())
        .filter(|id| !baseline.lanes.iter().any(|o| o.id == *id))
        .collect();
    added.sort_unstable();
    // PRD-mcphost-share-a-tool-not-a-key added its own "examples" lane
    // (routes examples/share-a-tool/** to the AC0x proof tests) -- a
    // second, intended addition since the baseline, not drift.
    assert_eq!(
        added,
        vec!["examples", "loop-config"],
        "only examples and loop-config may have been added since {BASELINE_REV}"
    );
}
