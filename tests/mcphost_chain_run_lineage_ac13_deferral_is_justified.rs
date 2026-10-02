//! PRD-mcphost-chain-run-lineage AC13 (P0, Live) -- AC13's Then is a
//! truth-tier `synthorg consume` run against prod after this PRD lands,
//! scoring `data-pipeline-builder-daily-pipeline-chain` >= 0.75: ~$5 and
//! ~40 min of real frontier calls billed to the operator's Anthropic key,
//! driven from another repository by a timer on a live host, against a
//! deployment this branch does not reach until the operator deploys it,
//! and gated on a second PRD (PRD-synthorg-truth-tier-probe-as-tenant)
//! this build role does not own. That is operator-provisioned work, and
//! no run in this worktree can perform or observe it; the always-on tests
//! in `tests/mcphost_chain_run_lineage_ac13_daily_pipeline_persona_trailer.rs`
//! prove the branch's own mechanism (schema-derived refusal, the retry,
//! the 3-child lineage read-back) plus conjunct 4's product-side factor
//! (the ported scorer's `accuracy`, 1.0 with this PRD's inlined `children`
//! and 0.5 without, which puts the 0.75 bar out of reach at every judge
//! label) -- and are explicitly NOT counted as AC13's proof.
//!
//! So the deferral this file locks is a NARROW one, and it has to say so:
//! the two factors left are the nightly judge's own `helped` label and the
//! persona's own wall clock, neither of which is a property of this
//! branch. The assertions below require the frontmatter and both agent
//! files to name the ported-scorer tests and the cross-repo synthorg
//! commit that carries the probe's runs-order half, so the narrowed claim
//! cannot quietly widen back into a bare "the whole score is deferred".
//!
//! What this file proves is that the deferral was declared honestly
//! rather than left as a bare "deferred" string in a JSON file: the PRD
//! frontmatter records `deferred_acs` including 13 and a concrete
//! `mock_justifications` entry naming the prod host, the credentials this
//! sandbox does not hold, and the `MCPHOST_LIVE=1` test fn that fails
//! until the operator's run has happened -- and `agent/test-map.json` /
//! `agent/intent-card.json` agree with that frontmatter. Same idiom as
//! `tests/docsqa_ac10_deferral_is_justified.rs`,
//! `tests/statusfeed_ac11_deferral_is_justified.rs` and
//! `tests/checkcompat_race_ac08_deferral_is_justified.rs`, except both
//! agent files are parsed as JSON (their AC entries are read by key, not
//! grepped -- a substring match cannot tell AC13's entry from AC1's).

use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const PRD: &str = "PRD-mcphost-chain-run-lineage.md";
const LIVE_TEST_FILE: &str = "tests/mcphost_chain_run_lineage_ac13_daily_pipeline_persona_trailer.rs";
const LIVE_TEST_FN: &str = "persona_two_call_sequence_yields_three_done_children";

/// The two always-on tests that carry conjunct 4's product-side factor --
/// synthorg's own scorer, ported and run against this branch's real
/// observation. Naming them in the paper trail is what makes the deferral
/// narrow (the judge's label and the persona's wall clock) rather than
/// "the score is a judge's, so all of conjunct 4 is deferred".
const SCORER_TEST_FNS: [&str; 2] = [
    "the_recipe_bar_is_unreachable_without_the_children_this_prd_inlines",
    "the_ported_scorer_matches_synthorgs_own_arithmetic",
];

/// The cross-repo half AC13's probe needs, landed in synthorg rather than
/// deferred as "another repository": `normalize_probed_runs` +
/// `_emit_host_context_probe(runs_order=...)`, committed on master and
/// recorded here by sha, the `checkcompat_race_ac08_deferral_is_justified`
/// idiom.
const SYNTHORG_COMMIT: &str = "c07c297";
const SYNTHORG_TEST_FILE: &str = "tests/chainlineage_ac13_probe_runs_order_test.py";

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
             agent/intent-card.json both cite it for AC13's deferral, so a dangling reference \
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
fn the_prd_is_committed_with_its_own_ac13_text() {
    let text = read(PRD);
    assert!(
        text.contains("- test_prefix: mcphost_chain_run_lineage"),
        "{PRD} must be this PRD, frontmatter and all"
    );
    let ac13 = text
        .lines()
        .find(|l| l.trim_start().starts_with("13. P0"))
        .unwrap_or_else(|| panic!("{PRD} has no AC13 line"));
    for needle in [
        "PRD-synthorg-truth-tier-probe-as-tenant",
        "synthorg consume --tier truth",
        "orch",
        "0.75",
    ] {
        assert!(ac13.contains(needle), "AC13's own text must name {needle:?}: {ac13}");
    }
}

#[test]
fn deferred_acs_frontmatter_names_ac13() {
    let text = read(PRD);
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("- deferred_acs:"))
        .unwrap_or_else(|| {
            panic!(
                "{PRD} frontmatter has no '- deferred_acs:' line; AC13 is deferred (its Then is a \
                 paid, operator-authorized truth-tier run against prod after land, gated on a \
                 second PRD) but that must be declared, not left implicit"
            )
        });
    let list = line
        .split_once(':')
        .map(|(_, rest)| rest.trim().trim_start_matches('[').trim_end_matches(']').to_string())
        .unwrap_or_default();
    let numbers: Vec<&str> = list.split(',').map(str::trim).collect();
    assert!(
        numbers.contains(&"13"),
        "deferred_acs must list AC13 as a number in list form (prd-lint parses prose as nothing): \
         {line:?}"
    );
}

#[test]
fn mock_justifications_names_ac13_with_a_concrete_reason() {
    let block = mock_justifications();

    assert!(block.contains("AC13"), "mock_justifications does not name AC13 by id: {block:?}");
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
        "orch",
        // the second PRD this AC is soft-coupled to
        "PRD-synthorg-truth-tier-probe-as-tenant",
    ] {
        assert!(
            block.contains(needle),
            "AC13's justification must name {needle:?} -- the concrete reason, not a vague \
             placeholder: {block:?}"
        );
    }
    assert!(
        block.contains(LIVE_TEST_FILE) && block.contains(LIVE_TEST_FN),
        "AC13's justification must name the live test file and the test fn that fails until the \
         operator's run has happened ({LIVE_TEST_FILE}::{LIVE_TEST_FN}): {block:?}"
    );
    assert!(
        block.contains("not counted as AC13's proof"),
        "AC13's justification must state the honest scope -- the always-on tests prove the \
         branch's mechanism, not AC13 itself: {block:?}"
    );

    // The narrowing itself, which is what makes this a deferral of two
    // named factors rather than of a whole conjunct: the scorer's own
    // arithmetic, the tests that run it against this branch's real
    // observation, and the cross-repo commit carrying the probe's half.
    for needle in [
        "timeliness * accuracy * helpfulness",
        "RECIPE_SATISFACTION_TARGET",
        "accuracy == 1.0",
        SCORER_TEST_FNS[0],
        SCORER_TEST_FNS[1],
        SYNTHORG_COMMIT,
        SYNTHORG_TEST_FILE,
    ] {
        assert!(
            block.contains(needle),
            "AC13's justification must name {needle:?} -- without it the deferral reads as \
             'the score comes from a judge, so all of conjunct 4 is deferred', which is not \
             what this branch proves: {block:?}"
        );
    }
}

#[test]
fn test_map_ac13_entry_agrees_with_the_frontmatter_deferral() {
    let map = read_json("agent/test-map.json");
    let is_current = map["ac_test_map_prd"].as_str().unwrap_or_default().split_whitespace().next()
        == Some(PRD);
    // agent/test-map.json's ac_test_map is refreshed to whichever PRD is
    // CURRENTLY building at HEAD (ac_test_map_contract documents this is
    // "not a repo-lifetime constant" -- same coupling the sibling
    // intent-card checks below already skip around). A later PRD's
    // legitimate refresh archives this PRD's map under
    // ac_test_map_by_prefix.mcphost_chain_run_lineage instead of deleting
    // it, so read from there once superseded -- same paper trail,
    // different shelf, not expected drift to skip past.
    let entry = if is_current {
        map["ac_test_map"]["AC13"].as_str().map(str::to_string)
    } else {
        map["ac_test_map_by_prefix"]["mcphost_chain_run_lineage"]["AC13"]
            .as_str()
            .map(str::to_string)
    }
    .expect(
        "agent/test-map.json must have an AC13 entry, current or archived under \
         mcphost_chain_run_lineage",
    );

    assert!(
        entry.starts_with("deferred"),
        "AC13's test-map entry must declare the deferral first: {entry:?}"
    );
    for needle in [
        "operator-provisioned",
        "mock_justifications",
        LIVE_TEST_FILE,
        LIVE_TEST_FN,
        SCORER_TEST_FNS[0],
        SCORER_TEST_FNS[1],
        SYNTHORG_COMMIT,
    ] {
        assert!(
            entry.contains(needle),
            "AC13's test-map entry must name {needle:?} so the deferral points at its own paper \
             trail instead of dangling: {entry:?}"
        );
    }
    if is_current {
        assert_eq!(
            map["ac_test_map_prd"].as_str().unwrap_or_default().split_whitespace().next(),
            Some(PRD),
            "agent/test-map.json's ac_test_map must be the map for this PRD"
        );
    }
}

#[test]
fn intent_card_ac13_entry_agrees_with_the_frontmatter_deferral() {
    let card = read_json("agent/intent-card.json");

    // agent/intent-card.json is refreshed on every wm-build run to name
    // whichever PRD is CURRENTLY building at HEAD (agent/test-map.json's
    // own ac_test_map_contract documents the pattern). A later PRD's
    // legitimate refresh rewrites the card and has no reason to preserve
    // this PRD's strings -- expected drift, not a paper-trail defect.
    let source = card["prd_source"].as_str().unwrap_or_default();
    if !source.contains("mcphost-chain-run-lineage") {
        eprintln!(
            "skip intent_card_ac13_entry_agrees_with_the_frontmatter_deferral: \
             agent/intent-card.json's prd_source is {source:?} -- a later PRD has refreshed the \
             card since (expected drift)"
        );
        return;
    }

    let ac13 = card["acceptance_criteria"]
        .as_array()
        .expect("acceptance_criteria array")
        .iter()
        .find(|ac| ac["id"] == "AC13")
        .unwrap_or_else(|| panic!("agent/intent-card.json has no AC13 entry"));
    let test = ac13["test"].as_str().expect("AC13 test field must be a string");

    assert!(
        test.starts_with("deferred"),
        "the card's AC13 test field must declare the deferral first: {test:?}"
    );
    for needle in [
        "operator-provisioned",
        "mock_justifications",
        LIVE_TEST_FILE,
        LIVE_TEST_FN,
        SCORER_TEST_FNS[0],
        SCORER_TEST_FNS[1],
        SYNTHORG_COMMIT,
    ] {
        assert!(
            test.contains(needle),
            "the card's AC13 test field must name {needle:?}: {test:?}"
        );
    }
}

/// The cross-repo half is LANDED, not deferred as "another repository":
/// `agent/test-map.json`'s AC13 entry names `/home/jsy/repos/synthorg`, a
/// commit sha and a `.py` proof, and where that checkout is reachable
/// (the operator's machine, not the gate's runner box) this resolves all
/// three for real -- the `checkcompat_race_ac08_deferral_is_justified`
/// idiom. Off that machine the pointer's own consistency is still
/// asserted, above and here.
#[test]
fn the_cross_repo_synthorg_half_is_landed_not_deferred() {
    let block = mock_justifications();
    assert!(
        block.contains("/home/jsy/repos/synthorg"),
        "the justification must name the synthorg checkout by absolute path: {block:?}"
    );
    assert!(
        block.contains("normalize_probed_runs"),
        "and the symbol that carries the probe's runs-order half: {block:?}"
    );

    let synthorg = PathBuf::from("/home/jsy/repos/synthorg");
    if !synthorg.join(".git").exists() {
        eprintln!(
            "skip the_cross_repo_synthorg_half_is_landed_not_deferred: no synthorg checkout at \
             {} (the gate's runner box does not carry it) -- the pointer's own consistency is \
             asserted above",
            synthorg.display()
        );
        return;
    }

    let show = std::process::Command::new("git")
        .args(["-C", "/home/jsy/repos/synthorg", "show", "--stat", "--format=%H", SYNTHORG_COMMIT])
        .output()
        .expect("git show in the synthorg checkout");
    assert!(
        show.status.success(),
        "synthorg commit {SYNTHORG_COMMIT} must exist in {}: {}",
        synthorg.display(),
        String::from_utf8_lossy(&show.stderr)
    );
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(
        stdout.contains(SYNTHORG_TEST_FILE),
        "commit {SYNTHORG_COMMIT} must be the one that carries {SYNTHORG_TEST_FILE}: {stdout}"
    );
    assert!(
        synthorg.join(SYNTHORG_TEST_FILE).exists(),
        "{SYNTHORG_TEST_FILE} must still be on master, not only in that commit"
    );
    let probe = fs::read_to_string(synthorg.join("src/synthorg/consume.py"))
        .expect("read synthorg's consume.py");
    assert!(
        probe.contains("def normalize_probed_runs(") && probe.contains("runs_order"),
        "the landed half must actually be in synthorg's probe, not just claimed in a sha"
    );
}
