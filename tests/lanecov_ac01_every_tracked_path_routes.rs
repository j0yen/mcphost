//! PRD-mcphost-proof-lane-loop-config, AC1/AC2/AC3/AC6 — a coverage proof
//! that reads `agent/proof-lanes.toml` and fails, naming every path it
//! names, when any path `git ls-files` reports resolves to zero lanes.
//!
//! Grounding: `.buildloop/ci-equivalent.toml` landed on `main` at commit
//! 4f1112d (the loop's own push-gate PRD) matching none of the nine lanes
//! that existed at the time, which sent `autobuilder vti-plan` red on
//! EVERY mcphost branch since 01:05Z (gate attribution `inherited=2
//! in-scope=0`) -- the gap was found by the gate, after the file was on
//! `main`, by a different PRD's dispatch. This file is what makes the next
//! unrouted top-level path a CI failure at PR time instead.
//!
//! Glob semantics deliberately mirror `agent/proof-lanes.toml`'s real
//! consumer: `autobuilder/autobuilder/src/vti_plan.rs` compiles each
//! lane's globs with the `globset` crate (`Glob::new` + `GlobSetBuilder`)
//! over `toml::from_str`-parsed lane specs. This file uses the same two
//! crates at the same major versions (see this crate's `Cargo.toml`) --
//! per this PRD's own Technical considerations note, if this test's
//! matcher and vti-plan's ever disagree, the test is wrong, not vti-plan.

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

/// One `[[lane]]` entry, deserialized straight from `agent/proof-lanes.toml`.
/// `note` is optional (only some lanes carry one) and otherwise unused here.
#[derive(Debug, Deserialize)]
struct Lane {
    id: String,
    #[allow(dead_code)]
    description: String,
    globs: Vec<String>,
    required_commands: Vec<String>,
    #[allow(dead_code)]
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LaneFile {
    #[serde(rename = "lane", default)]
    lanes: Vec<Lane>,
}

/// Paths that intentionally route to no lane. This PRD's own scope is
/// exactly one gap (`.buildloop/**`, folded into the new `loop-config`
/// lane below) and its AC4 requires `agent/proof-lanes.toml` to diff
/// against v0.54.1 as ONLY that one added `[[lane]]` block -- no existing
/// lane's globs may widen to cover the two entries below, pre-existing
/// debt this test surfaces but this PRD does not fix (Non-goals: "No
/// change to what any existing lane requires"). Route these with a real
/// lane in a follow-on PRD rather than growing this list further.
const UNROUTED_ALLOWLIST: &[&str] = &[
    // Repo-hygiene file, same conceptual class as .gitignore (which the
    // "meta" lane already covers) but not itself in that lane's globs.
    ".gitattributes",
    // Prose doc adjacent to docs/kinds/** and docs/receipts/** (which the
    // "docs" lane covers) but not itself in that lane's globs.
    "docs/audit-notes.md",
];

/// Compiles one lane's globs into a matchable set, or an error naming the
/// lane id and the bad glob (P1 requirement 4 / AC6's "malformed glob"
/// half).
fn compile_lane_globset(lane: &Lane) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for g in &lane.globs {
        let glob = Glob::new(g)
            .map_err(|e| format!("lane {} has invalid glob {g:?}: {e}", lane.id))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|e| format!("lane {} failed to compile its globset: {e}", lane.id))
}

/// P1 requirement 4: every lane's `required_commands` must be non-empty
/// and every lane's globs must compile, so a malformed lane fails this
/// test instead of silently never matching or reporting confidence 0 at
/// vti-plan time. Returns the first error, naming the lane id.
fn validate_lane_shapes(lanes: &[Lane]) -> Result<Vec<GlobSet>, String> {
    let mut sets = Vec::with_capacity(lanes.len());
    for lane in lanes {
        if lane.required_commands.is_empty() {
            return Err(format!("lane {} has empty required_commands", lane.id));
        }
        sets.push(compile_lane_globset(lane)?);
    }
    Ok(sets)
}

/// Returns every path in `paths` that no `globset` in `lane_sets` matches,
/// excluding anything in `allowlist`. This is the reusable core AC1's real
/// coverage test and AC3's synthetic self-test both exercise, so the two
/// scenarios (real repo tree vs. a fabricated unrouted file) can never
/// disagree about what "unrouted" means.
fn find_unrouted<'a>(
    lane_sets: &[GlobSet],
    paths: &'a [String],
    allowlist: &[&str],
) -> Vec<&'a str> {
    paths
        .iter()
        .map(String::as_str)
        .filter(|p| !allowlist.contains(p))
        .filter(|p| !lane_sets.iter().any(|set| set.is_match(p)))
        .collect()
}

fn load_lane_file(manifest_dir: &Path) -> LaneFile {
    let text = std::fs::read_to_string(manifest_dir.join("agent/proof-lanes.toml"))
        .expect("agent/proof-lanes.toml must exist");
    toml::from_str(&text).expect("agent/proof-lanes.toml must be valid TOML matching the Lane schema")
}

/// `Ok(None)` means this checkout has no `.git` to ask at all (observed in
/// the wild: a burst-lane gate producer runs against an rsynced copy of
/// the worktree with `--exclude .git`, per that tool's own sync contract —
/// there is no tracked-file list to prove coverage against there, so the
/// only honest answer is "cannot check here", never a false pass or a
/// false unrouted-path report). Any OTHER git failure is still a hard
/// error — this narrows only the specific "no repository" case, not git
/// failing for some other reason.
fn git_ls_files(manifest_dir: &Path) -> Result<Option<Vec<String>>, String> {
    let output = Command::new("git")
        .arg("ls-files")
        .current_dir(manifest_dir)
        .output()
        .map_err(|e| format!("git ls-files could not be run: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            return Ok(None);
        }
        return Err(format!("git ls-files exited non-zero: {stderr}"));
    }
    let paths = String::from_utf8(output.stdout)
        .map_err(|e| format!("git ls-files output must be UTF-8: {e}"))?
        .lines()
        .map(str::to_owned)
        .filter(|l| !l.is_empty())
        .collect();
    Ok(Some(paths))
}

/// AC1/AC2: every path `git ls-files` reports must resolve to at least one
/// lane in `agent/proof-lanes.toml`. On failure, names every unrouted path
/// so the fix (add a lane, or add to the allowlist with a reason) is
/// obvious from the test output alone. Skips (prints, does not fail) only
/// when this checkout has no `.git` at all to ask — see `git_ls_files`'s
/// own doc comment; every environment with a real `.git` (every real dev
/// checkout, every CI runner) still runs the real proof.
#[test]
fn lanecov_ac01_every_tracked_path_routes() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lane_file = load_lane_file(manifest_dir);
    assert!(
        !lane_file.lanes.is_empty(),
        "agent/proof-lanes.toml has no [[lane]] entries; nothing to route against"
    );
    let lane_sets = validate_lane_shapes(&lane_file.lanes)
        .expect("every lane in agent/proof-lanes.toml must have non-empty required_commands and compilable globs");

    let paths = match git_ls_files(manifest_dir) {
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

/// P1 requirement 4 / AC6: a lane with an empty `required_commands` must
/// fail the proof, naming the lane id -- checked against a synthetic lane
/// so this test never depends on the shipped file staying broken (or
/// staying fixed) to prove the check works.
#[test]
fn lanecov_lane_with_empty_required_commands_fails_naming_the_lane() {
    let lanes = vec![Lane {
        id: "broken-lane".to_string(),
        description: "synthetic".to_string(),
        globs: vec!["synthetic/**".to_string()],
        required_commands: vec![],
        note: None,
    }];
    let err = validate_lane_shapes(&lanes).expect_err("empty required_commands must fail validation");
    assert!(err.contains("broken-lane"), "error must name the lane id: {err}");
}

/// P1 requirement 4's other half: a lane whose glob does not compile must
/// fail the proof, naming the lane id, rather than vti-plan discovering it
/// later as a `GlobSetBuilder` error with no PRD context.
#[test]
fn lanecov_lane_with_uncompilable_glob_fails_naming_the_lane() {
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
    assert!(err.contains("unbalanced-lane"), "error must name the lane id: {err}");
}

/// AC3: a tracked path with no matching lane must be named in the
/// failure. Exercised against a synthetic lane set + path list (the
/// reusable `find_unrouted` core AC1 itself calls) rather than a real
/// scratch git clone, since the two scenarios can never disagree by
/// construction -- see `find_unrouted`'s own doc comment.
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
    let paths = vec![
        "src/main.rs".to_string(),
        "zz-unrouted/x.txt".to_string(),
    ];
    let unrouted = find_unrouted(&lane_sets, &paths, &[]);
    assert_eq!(
        unrouted,
        vec!["zz-unrouted/x.txt"],
        "the synthetic unrouted path must be named and nothing else"
    );
}

/// Sanity check the allowlist mechanism itself: an allowlisted path is
/// excluded from the unrouted report even when no lane covers it.
#[test]
fn lanecov_allowlisted_path_is_excluded() {
    let lanes = vec![Lane {
        id: "src-only".to_string(),
        description: "synthetic".to_string(),
        globs: vec!["src/**".to_string()],
        required_commands: vec!["cargo test --workspace".to_string()],
        note: None,
    }];
    let lane_sets = validate_lane_shapes(&lanes).expect("synthetic lane must validate");
    let paths = vec!["ALLOWED_STRAY_FILE".to_string()];
    let unrouted = find_unrouted(&lane_sets, &paths, &["ALLOWED_STRAY_FILE"]);
    assert!(unrouted.is_empty(), "an allowlisted path must never be reported: {unrouted:?}");
}
