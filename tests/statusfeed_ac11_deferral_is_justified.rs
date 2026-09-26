//! PRD-mcphost-status-feed AC11 (P0, Live) -- the prod-after-land leg of
//! AC11 (a live deploy to `https://mcphost.dev`) is not something this
//! coding sandbox can execute; deploying is an operator action. The AC's
//! own always-on mechanism proof lives in
//! `tests/statusfeed_ac11_live_status_trailer.rs`, gated on `MCPHOST_LIVE=1`
//! for the real leg. What this file proves is that the deferral of the
//! *prod* half was declared honestly, not silently: the PRD frontmatter
//! records `deferred_acs: [11]` and a concrete `mock_justifications` entry
//! naming AC11, and `agent/test-map.json` / `agent/intent-card.json` agree
//! with that frontmatter instead of dangling a bare "deferred" string (the
//! exact defect `tests/checkcompat_race_ac08_deferral_is_justified.rs`
//! already locked for a different PRD).

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn prd_text() -> String {
    let path = repo_root().join("PRD-mcphost-status-feed.md");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn deferred_acs_frontmatter_names_ac11() {
    let text = prd_text();
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("- deferred_acs:"))
        .unwrap_or_else(|| {
            panic!(
                "PRD frontmatter has no '- deferred_acs:' line; AC11 is deferred (its prod \
                 leg needs a real deploy to https://mcphost.dev this sandbox cannot perform) \
                 but that must be declared, not left implicit"
            )
        });
    assert!(
        line.contains("11"),
        "PRD frontmatter's deferred_acs line does not list AC11: {line:?}"
    );
}

#[test]
fn mock_justifications_names_ac11_with_a_concrete_reason() {
    let text = prd_text();
    let idx = text.find("mock_justifications:").unwrap_or_else(|| {
        panic!(
            "PRD frontmatter has no 'mock_justifications:' field; agent/test-map.json and \
             agent/intent-card.json both must cite it for AC11's deferral, so a dangling \
             reference is a broken paper trail, not a valid one"
        )
    });
    let rest = &text[idx..];
    let justification_block = &rest[..rest.find("\n- ").unwrap_or(rest.len())];

    assert!(
        justification_block.contains("AC11"),
        "mock_justifications does not name AC11 by id: {justification_block:?}"
    );
    assert!(
        justification_block.contains("mcphost.dev") && justification_block.contains("deploy"),
        "AC11's justification must name the concrete reason (a real deploy to \
         mcphost.dev this sandbox cannot perform), not a vague placeholder: \
         {justification_block:?}"
    );
    assert!(
        justification_block.contains("statusfeed_ac11_live_status_trailer.rs"),
        "AC11's justification must name the live test file that carries the \
         always-on mechanism proof: {justification_block:?}"
    );
    assert!(
        justification_block.contains("not counted as AC11's proof"),
        "AC11's justification must state the honest scope -- the always-on test proves \
         the branch's mechanism, not AC11 itself: {justification_block:?}"
    );
}

#[test]
fn test_map_and_intent_card_ac11_entries_agree_with_the_frontmatter_deferral() {
    let test_map_path = repo_root().join("agent/test-map.json");
    let test_map = fs::read_to_string(&test_map_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", test_map_path.display()));
    assert!(
        test_map.contains("\"AC11\"") && test_map.contains("mock_justifications"),
        "agent/test-map.json's AC11 entry must point at mock_justifications now that the \
         PRD frontmatter actually carries that field"
    );

    // agent/intent-card.json is refreshed on every wm-build run to name
    // whichever PRD is CURRENTLY building at HEAD (agent/test-map.json's own
    // ac_test_map_contract documents the identical pattern). This assertion
    // only holds while mcphost-status-feed is still that PRD; a later PRD's
    // legitimate refresh rewrites the card to its own content and has no
    // reason to preserve this string -- skip with a printed reason rather
    // than fail on expected drift (same fix shape as
    // tests/checkcompat_race_ac08_deferral_is_justified.rs's own test).
    let intent_card_path = repo_root().join("agent/intent-card.json");
    let intent_card = fs::read_to_string(&intent_card_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", intent_card_path.display()));
    if !intent_card.contains("PRD-mcphost-status-feed") {
        eprintln!(
            "skip test_map_and_intent_card_ac11_entries_agree_with_the_frontmatter_deferral: \
             agent/intent-card.json no longer names PRD-mcphost-status-feed -- a later PRD \
             has refreshed the card since (expected drift), not a paper-trail defect"
        );
        return;
    }
    assert!(
        intent_card.contains("mock_justifications"),
        "agent/intent-card.json's AC11 test field must point at mock_justifications now \
         that the PRD frontmatter actually carries that field"
    );
}
