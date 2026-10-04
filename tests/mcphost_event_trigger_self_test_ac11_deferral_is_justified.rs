//! PRD-mcphost-event-trigger-self-test AC11 (P0, Live) -- AC11's Then is a
//! nightly `synthorg consume --tier truth` run on orch against prod after
//! deploy: real frontier calls billed to the operator's Anthropic key,
//! driven from another repository by a timer on a live host, against a
//! deployment this branch does not reach until the operator deploys it.
//! That is operator-provisioned work, and no run in this worktree can
//! perform or observe it; the always-on tests in
//! `tests/mcphost_event_trigger_self_test_ac11_truth_tier_persona_reaches_event_run.rs`
//! prove the branch's own half (the persona path, replayed; the live
//! check's bar, exercised on the real pre-feature nightly) and are
//! explicitly NOT counted as AC11's proof.
//!
//! What this file proves is that the deferral was declared honestly rather
//! than left as a bare "deferred" string in a JSON file: the PRD is
//! committed here with its own AC11 text, its frontmatter records
//! `deferred_acs: [11]` and a concrete `mock_justifications` entry naming
//! the prod host, the credentials this sandbox does not hold, and the
//! `MCPHOST_LIVE=1` test fn that fails until the operator's nightly has
//! happened -- and `agent/test-map.json` / `agent/intent-card.json` agree
//! with that frontmatter. Same idiom as
//! `tests/docsqa_ac10_deferral_is_justified.rs`, with both agent files
//! parsed as JSON (their AC entries read by key, not grepped -- a
//! substring match cannot tell AC11's entry from AC1's).

use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const PRD: &str = "PRD-mcphost-event-trigger-self-test.md";
const LIVE_TEST_FILE: &str =
    "tests/mcphost_event_trigger_self_test_ac11_truth_tier_persona_reaches_event_run.rs";
const LIVE_TEST_FN: &str = "live_truth_tier_persona_run_meets_the_target";

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
/// frontmatter bullet (`\n- `) -- continuation lines that do not start a
/// new bullet belong to the field.
fn mock_justifications() -> String {
    let text = read(PRD);
    let idx = text.find("mock_justifications:").unwrap_or_else(|| {
        panic!(
            "{PRD} frontmatter has no 'mock_justifications:' field; agent/test-map.json and \
             agent/intent-card.json both cite it for AC11's deferral, so a dangling reference \
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
fn the_prd_is_committed_with_its_own_ac11_text() {
    let text = read(PRD);
    assert!(
        text.contains("host.trigger.test") && text.contains("event-trigger-self-test"),
        "{PRD} must be this PRD, frontmatter and all"
    );
    let ac11 = text
        .lines()
        .find(|l| l.trim_start().starts_with("11. P0"))
        .unwrap_or_else(|| panic!("{PRD} has no AC11 line"));
    for needle in [
        "PRD-synthorg-truth-tier-probe-as-tenant",
        "integration-specialist-github-push-webhook-handler",
        "host.trigger.test",
        "0.75",
        "synthorg consume --tier truth",
    ] {
        assert!(ac11.contains(needle), "AC11's own text must name {needle:?}: {ac11}");
    }
}

#[test]
fn deferred_acs_frontmatter_names_ac11() {
    let text = read(PRD);
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("- deferred_acs:"))
        .unwrap_or_else(|| {
            panic!(
                "{PRD} frontmatter has no '- deferred_acs:' line; AC11 is deferred (its Then is a \
                 paid, operator-authorized truth-tier nightly against prod after deploy) but that \
                 must be declared, not left implicit"
            )
        });
    let list = line
        .split_once(':')
        .map(|(_, rest)| rest.trim().trim_start_matches('[').trim_end_matches(']').to_string())
        .unwrap_or_default();
    let numbers: Vec<&str> = list.split(',').map(str::trim).collect();
    assert!(
        numbers.contains(&"11"),
        "deferred_acs must list AC11 as a number in list form (prd-lint parses prose as nothing): \
         {line:?}"
    );
}

#[test]
fn mock_justifications_names_ac11_with_a_concrete_reason() {
    let block = mock_justifications();

    assert!(block.contains("AC11"), "mock_justifications does not name AC11 by id: {block:?}");
    for needle in [
        // the real host the nightly needs, and what makes the run itself paid
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
            "AC11's justification must name {needle:?} -- the concrete reason, not a vague \
             placeholder: {block:?}"
        );
    }
    assert!(
        block.contains(LIVE_TEST_FILE) && block.contains(LIVE_TEST_FN),
        "AC11's justification must name the live test file and the test fn that fails until the \
         operator's nightly has happened ({LIVE_TEST_FILE}::{LIVE_TEST_FN}): {block:?}"
    );
    assert!(
        block.contains("not counted as AC11's proof"),
        "AC11's justification must state the honest scope -- the always-on tests prove the \
         branch's own half, not AC11 itself: {block:?}"
    );
}

/// The live test fn the paper trail points at has to exist, be gated on
/// `MCPHOST_LIVE=1`, and be the only test in its file that can claim AC11.
#[test]
fn the_live_test_fn_exists_and_is_gated_on_mcphost_live() {
    let text = read(LIVE_TEST_FILE);
    assert!(
        text.contains(&format!("fn {LIVE_TEST_FN}(")),
        "{LIVE_TEST_FILE} must define {LIVE_TEST_FN} -- every other file in this paper trail \
         names it"
    );
    let body = text
        .split_once(&format!("fn {LIVE_TEST_FN}("))
        .map(|(_, rest)| rest)
        .unwrap_or_default();
    assert!(
        body.contains("MCPHOST_LIVE"),
        "{LIVE_TEST_FN} must skip unless MCPHOST_LIVE=1: it cannot run in this sandbox at all"
    );
}

#[test]
fn test_map_ac11_entry_agrees_with_the_frontmatter_deferral() {
    let map = read_json("agent/test-map.json");

    // agent/test-map.json's own ac_test_map_contract documents the
    // "current PRD at HEAD, not a repo-lifetime constant" pattern: a later
    // PRD's legitimate refresh repoints ac_test_map at its own ACs
    // (archiving this PRD's own map under
    // ac_test_map_by_prefix.mcphost_event_trigger_self_test) and has no
    // reason to preserve this PRD's strings at the live top-level key.
    // Check the archived copy instead once that's happened -- same paper
    // trail, different address.
    let prd_name = map["ac_test_map_prd"].as_str().unwrap_or_default();
    let entry = if prd_name.contains("mcphost-event-trigger-self-test") {
        map["ac_test_map"]["AC11"]
            .as_str()
            .expect("agent/test-map.json ac_test_map.AC11 must be a string")
            .to_string()
    } else {
        eprintln!(
            "test_map_ac11_entry_agrees_with_the_frontmatter_deferral: agent/test-map.json's \
             ac_test_map_prd is {prd_name:?} -- a later PRD has refreshed it since (expected \
             drift); checking the archived map instead"
        );
        map["ac_test_map_by_prefix"]["mcphost_event_trigger_self_test"]["AC11"]
            .as_str()
            .expect(
                "agent/test-map.json ac_test_map_by_prefix.mcphost_event_trigger_self_test.AC11 \
                 must be a string",
            )
            .to_string()
    };

    assert!(
        entry.starts_with("deferred"),
        "AC11's test-map entry must declare the deferral first: {entry:?}"
    );
    for needle in ["operator-provisioned", "mock_justifications", LIVE_TEST_FILE, LIVE_TEST_FN] {
        assert!(
            entry.contains(needle),
            "AC11's test-map entry must name {needle:?} so the deferral points at its own paper \
             trail instead of dangling: {entry:?}"
        );
    }
}

#[test]
fn intent_card_ac11_entry_agrees_with_the_frontmatter_deferral() {
    let card = read_json("agent/intent-card.json");

    // Refreshed on every wm-build run to name whichever PRD is CURRENTLY
    // building at HEAD (see agent/test-map.json's ac_test_map_contract) --
    // a later PRD's refresh rewriting this card is expected drift, not a
    // paper-trail defect.
    let source = card["prd_source"].as_str().unwrap_or_default();
    if !source.contains("mcphost-event-trigger-self-test") {
        eprintln!(
            "skip intent_card_ac11_entry_agrees_with_the_frontmatter_deferral: \
             agent/intent-card.json's prd_source is {source:?} -- a later PRD has refreshed the \
             card since (expected drift)"
        );
        return;
    }

    let ac11 = card["acceptance_criteria"]
        .as_array()
        .expect("acceptance_criteria array")
        .iter()
        .find(|ac| ac["id"] == "AC11")
        .unwrap_or_else(|| panic!("agent/intent-card.json has no AC11 entry"));
    let test = ac11["test"].as_str().expect("AC11 test field must be a string");

    assert!(
        test.starts_with("deferred"),
        "the card's AC11 test field must declare the deferral first: {test:?}"
    );
    for needle in ["operator-provisioned", "mock_justifications", LIVE_TEST_FILE, LIVE_TEST_FN] {
        assert!(
            test.contains(needle),
            "the card's AC11 test field must name {needle:?}: {test:?}"
        );
    }
}
