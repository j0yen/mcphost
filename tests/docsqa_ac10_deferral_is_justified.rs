//! PRD-mcphost-docs-qa-recipe AC10 (P0, Live) -- AC10's Then is a truth-tier
//! `synthorg consume` run against prod after land: ~$5 and ~40 min of real
//! frontier calls billed to the operator's Anthropic key, driven from another
//! repository by a timer on a live host, against a deployment this branch
//! does not reach until the operator deploys it. That is operator-provisioned
//! work, and no run in this worktree can perform or observe it; the always-on
//! tests in `tests/docsqa_ac10_truth_tier_panel_satisfaction_recorded.rs`
//! prove the branch's own mechanism (the recorder) and are explicitly NOT
//! counted as AC10's proof.
//!
//! What this file proves is that the deferral was declared honestly rather
//! than left as a bare "deferred" string in a JSON file: the PRD frontmatter
//! records `deferred_acs` including 10 and a concrete `mock_justifications`
//! entry naming the prod host, the credentials this sandbox does not hold,
//! and the `MCPHOST_LIVE=1` test fn that fails until the operator's run has
//! happened -- and `agent/test-map.json` / `agent/intent-card.json` agree with
//! that frontmatter. Same idiom as
//! `tests/statusfeed_ac11_deferral_is_justified.rs` and
//! `tests/checkcompat_race_ac08_deferral_is_justified.rs`, except both agent
//! files are parsed as JSON (their AC entries are read by key, not
//! grepped -- a substring match cannot tell AC10's entry from AC1's).

use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const PRD: &str = "PRD-mcphost-docs-qa-recipe.md";
const LIVE_TEST_FILE: &str = "tests/docsqa_ac10_truth_tier_panel_satisfaction_recorded.rs";
const LIVE_TEST_FN: &str = "live_truth_tier_panel_run_meets_the_target";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn read_json(rel: &str) -> Value {
    serde_json::from_str(&read(rel)).unwrap_or_else(|e| panic!("parse {rel}: {e}"))
}

/// The `mock_justifications:` frontmatter field, from its key to the next
/// frontmatter bullet (`\n- `) -- continuation lines that do not start a new
/// bullet belong to the field.
fn mock_justifications() -> String {
    let text = read(PRD);
    let idx = text.find("mock_justifications:").unwrap_or_else(|| {
        panic!(
            "{PRD} frontmatter has no 'mock_justifications:' field; agent/test-map.json and \
             agent/intent-card.json both cite it for AC10's deferral, so a dangling reference \
             would be a broken paper trail, not a valid one"
        )
    });
    let rest = &text[idx..];
    rest[..rest.find("\n- ").unwrap_or(rest.len())].to_string()
}

/// The PRD this build is for has to be in the repo at all for the paper
/// trail to be checkable on the gate's runner box, where
/// `~/Documents/PRDs/build-queue` does not exist.
#[test]
fn the_prd_is_committed_with_its_own_ac10_text() {
    let text = read(PRD);
    assert!(
        text.contains("- test_prefix: docsqa"),
        "{PRD} must be this PRD, frontmatter and all"
    );
    let ac10 = text
        .lines()
        .find(|l| l.trim_start().starts_with("10. P0"))
        .unwrap_or_else(|| panic!("{PRD} has no AC10 line"));
    for needle in ["truth-tier panel run after land", "synthorg consume", "rag_indexer", "75%"] {
        assert!(ac10.contains(needle), "AC10's own text must name {needle:?}: {ac10}");
    }
}

#[test]
fn deferred_acs_frontmatter_names_ac10() {
    let text = read(PRD);
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("- deferred_acs:"))
        .unwrap_or_else(|| {
            panic!(
                "{PRD} frontmatter has no '- deferred_acs:' line; AC10 is deferred (its Then is a \
                 paid, operator-authorized truth-tier panel run against prod after land) but that \
                 must be declared, not left implicit"
            )
        });
    let list = line
        .split_once(':')
        .map(|(_, rest)| rest.trim().trim_start_matches('[').trim_end_matches(']').to_string())
        .unwrap_or_default();
    let numbers: Vec<&str> = list.split(',').map(str::trim).collect();
    assert!(
        numbers.contains(&"10"),
        "deferred_acs must list AC10 as a number in list form (prd-lint parses prose as nothing): \
         {line:?}"
    );
}

#[test]
fn mock_justifications_names_ac10_with_a_concrete_reason() {
    let block = mock_justifications();

    assert!(block.contains("AC10"), "mock_justifications does not name AC10 by id: {block:?}");
    for needle in [
        // the real host the run needs, and what makes the run itself paid
        "mcphost.dev",
        "--tier truth",
        // the concrete credentials this sandbox does not hold
        "ANTHROPIC_API_KEY",
        "SYNTHORG_PROD_ENDPOINT",
        // where the run lives, and why it is the operator's action
        "~/repos/synthorg",
        "operator",
    ] {
        assert!(
            block.contains(needle),
            "AC10's justification must name {needle:?} -- the concrete reason, not a vague \
             placeholder: {block:?}"
        );
    }
    assert!(
        block.contains(LIVE_TEST_FILE) && block.contains(LIVE_TEST_FN),
        "AC10's justification must name the live test file and the test fn that fails until the \
         operator's run has happened ({LIVE_TEST_FILE}::{LIVE_TEST_FN}): {block:?}"
    );
    assert!(
        block.contains("not counted as AC10's proof"),
        "AC10's justification must state the honest scope -- the always-on tests prove the \
         branch's mechanism, not AC10 itself: {block:?}"
    );
}

#[test]
fn test_map_ac10_entry_agrees_with_the_frontmatter_deferral() {
    let map = read_json("agent/test-map.json");
    let entry = map["ac_test_map"]["AC10"]
        .as_str()
        .expect("agent/test-map.json ac_test_map.AC10 must be a string");

    assert!(
        entry.starts_with("deferred"),
        "AC10's test-map entry must declare the deferral first: {entry:?}"
    );
    for needle in ["operator-provisioned", "mock_justifications", LIVE_TEST_FILE, LIVE_TEST_FN] {
        assert!(
            entry.contains(needle),
            "AC10's test-map entry must name {needle:?} so the deferral points at its own paper \
             trail instead of dangling: {entry:?}"
        );
    }
    assert_eq!(
        map["ac_test_map_prd"].as_str().unwrap_or_default().split_whitespace().next(),
        Some(PRD),
        "agent/test-map.json's ac_test_map must be the map for this PRD"
    );
}

#[test]
fn intent_card_ac10_entry_agrees_with_the_frontmatter_deferral() {
    let card = read_json("agent/intent-card.json");

    // agent/intent-card.json is refreshed on every wm-build run to name
    // whichever PRD is CURRENTLY building at HEAD (agent/test-map.json's own
    // ac_test_map_contract documents the pattern). A later PRD's legitimate
    // refresh rewrites the card and has no reason to preserve this PRD's
    // strings -- expected drift, not a paper-trail defect.
    let source = card["prd_source"].as_str().unwrap_or_default();
    if !source.contains("mcphost-docs-qa-recipe") {
        eprintln!(
            "skip intent_card_ac10_entry_agrees_with_the_frontmatter_deferral: \
             agent/intent-card.json's prd_source is {source:?} -- a later PRD has refreshed the \
             card since (expected drift)"
        );
        return;
    }

    let ac10 = card["acceptance_criteria"]
        .as_array()
        .expect("acceptance_criteria array")
        .iter()
        .find(|ac| ac["id"] == "AC10")
        .unwrap_or_else(|| panic!("agent/intent-card.json has no AC10 entry"));
    let test = ac10["test"].as_str().expect("AC10 test field must be a string");

    assert!(
        test.starts_with("deferred"),
        "the card's AC10 test field must declare the deferral first: {test:?}"
    );
    for needle in ["operator-provisioned", "mock_justifications", LIVE_TEST_FILE, LIVE_TEST_FN] {
        assert!(
            test.contains(needle),
            "the card's AC10 test field must name {needle:?}: {test:?}"
        );
    }
}
