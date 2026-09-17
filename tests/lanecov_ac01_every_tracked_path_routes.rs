//! PRD-mcphost-proof-lane-loop-config, AC1 — every path `git ls-files`
//! reports resolves to at least one lane in `agent/proof-lanes.toml`.
//!
//! Grounding: `.buildloop/ci-equivalent.toml` landed on `main` at commit
//! 4f1112d (the loop's own push-gate PRD) matching none of the nine lanes
//! that existed at the time, which sent `autobuilder vti-plan` red on EVERY
//! mcphost branch since 01:05Z (gate attribution `inherited=2 in-scope=0`)
//! -- the gap was found by the gate, after the file was on `main`, by a
//! different PRD's dispatch. This file is what makes the next unrouted
//! top-level path a CI failure at PR time instead.
//!
//! The shared matcher lives in `tests/support/lanecov.rs`; each of this
//! PRD's ACs owns its own `tests/lanecov_ac<N>_*.rs` file so the archive
//! gate's AC->test derivation can pair them one-for-one.

#[path = "support/lanecov.rs"]
mod lanecov;

use lanecov::{
    find_unrouted, git_ls_files, load_lane_file, manifest_dir, validate_lane_shapes,
    UNROUTED_ALLOWLIST,
};

/// AC1: every tracked path must resolve to at least one lane. On failure,
/// names every unrouted path so the fix (add a lane, or add to
/// `UNROUTED_ALLOWLIST` with a reason) is obvious from the output alone.
/// Skips (prints, does not fail) only when this checkout has no `.git` at
/// all to ask -- see `git_ls_files`'s own doc comment; every environment
/// with a real `.git` still runs the real proof.
#[test]
fn lanecov_ac01_every_tracked_path_routes() {
    let dir = manifest_dir();
    let lane_file = load_lane_file(dir);
    assert!(
        !lane_file.lanes.is_empty(),
        "agent/proof-lanes.toml has no [[lane]] entries; nothing to route against"
    );
    let lane_sets = validate_lane_shapes(&lane_file.lanes).expect(
        "every lane in agent/proof-lanes.toml must have non-empty required_commands and \
         compilable globs",
    );

    let paths = match git_ls_files(dir) {
        Ok(Some(paths)) => paths,
        Ok(None) => {
            println!(
                "lanecov_ac01_every_tracked_path_routes: skipped — no .git in this checkout \
                 (e.g. a gate producer's rsynced sandbox), so there is no tracked-file list to \
                 prove coverage against; this test runs for real in any checkout that has one"
            );
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let unrouted = find_unrouted(&lane_sets, &paths, UNROUTED_ALLOWLIST);
    assert!(
        unrouted.is_empty(),
        "the following tracked path(s) match no lane in agent/proof-lanes.toml \
         (add a lane, or add to UNROUTED_ALLOWLIST with a reason): {unrouted:?}"
    );
}

/// AC1's own guardrail: `.buildloop/ci-equivalent.toml` -- the exact path
/// whose absence from every lane caused the repo-wide red -- must route,
/// and the `loop-config` lane this PRD added must be one of the lanes that
/// routes it. A later PRD folding the same glob into another lane is fine
/// (that happened: gate-debt-4f1112d widened `meta`); dropping
/// `loop-config`'s own route is not.
#[test]
fn lanecov_ac01_buildloop_config_routes_to_loop_config_lane() {
    let dir = manifest_dir();
    let lane_file = load_lane_file(dir);
    let loop_config = lane_file
        .lanes
        .iter()
        .find(|l| l.id == "loop-config")
        .expect("agent/proof-lanes.toml must carry the loop-config lane");
    let set = lanecov::compile_lane_globset(loop_config).expect("loop-config globs must compile");
    assert!(
        set.is_match(".buildloop/ci-equivalent.toml"),
        "loop-config must route .buildloop/ci-equivalent.toml; globs were {:?}",
        loop_config.globs
    );
    assert!(
        !loop_config.required_commands.is_empty(),
        "loop-config must declare at least one required command"
    );
}

/// Sanity check the allowlist mechanism itself: an allowlisted path is
/// excluded from the unrouted report even when no lane covers it.
#[test]
fn lanecov_allowlisted_path_is_excluded() {
    let lanes = vec![lanecov::Lane {
        id: "src-only".to_string(),
        description: "synthetic".to_string(),
        globs: vec!["src/**".to_string()],
        required_commands: vec!["cargo test --workspace".to_string()],
        note: None,
    }];
    let lane_sets = validate_lane_shapes(&lanes).expect("synthetic lane must validate");
    let paths = vec!["ALLOWED_STRAY_FILE".to_string()];
    let unrouted = find_unrouted(&lane_sets, &paths, &["ALLOWED_STRAY_FILE"]);
    assert!(
        unrouted.is_empty(),
        "an allowlisted path must never be reported: {unrouted:?}"
    );
}
