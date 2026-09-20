//! PRD-mcphost-checkcompat-port-race AC8 (P0) — Given a real ccx53 burst
//! box (deferrable only with a justification naming why no box was
//! reachable; `mock_justifications` required), When `burst-lane prove`
//! runs on this PRD's HEAD, Then `prove done routed=true` and the run log
//! shows the ac02/ac03 tests `ok`.
//!
//! This coding sandbox has no `HCLOUD_TOKEN` (confirmed live: hitting
//! Hetzner's API returns 401, not a network failure, and `burst-lane
//! status` shows no active session), and the installed `burst-lane` CLI's
//! real subcommands (status/cost/why-down/sub-cap/route-check/evidence/
//! keep-check) carry no `prove` verb to invoke even if a box were up -- so
//! this AC's real-box proof cannot run from here. The AC's own text builds
//! in that exact escape hatch ("deferrable only with a justification...
//! mock_justifications required"), so what this file proves instead is
//! that the escape hatch was actually taken honestly, not silently: the
//! PRD frontmatter records `deferred_acs: [8]` and a concrete, checkable
//! `mock_justifications` reason, and the AC-to-test pointers that cite it
//! (`agent/test-map.json`, `agent/intent-card.json`) actually agree with
//! that frontmatter instead of dangling. That dangling reference -- both
//! files said "See PRD frontmatter mock_justifications" while the PRD
//! carried no such field -- is the exact defect this file locks against
//! regressing.

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn prd_text() -> String {
    let path = repo_root().join("PRD-mcphost-checkcompat-port-race.md");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn deferred_acs_frontmatter_names_ac8() {
    let text = prd_text();
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("- deferred_acs:"))
        .unwrap_or_else(|| {
            panic!(
                "PRD frontmatter has no '- deferred_acs:' line; AC8 is deferred (no \
                 ccx53 burst box reachable from this sandbox) but that must be declared, \
                 not left implicit"
            )
        });
    assert!(
        line.contains('8'),
        "PRD frontmatter's deferred_acs line does not list AC8: {line:?}"
    );
}

#[test]
fn mock_justifications_names_ac8_with_a_concrete_reason() {
    let text = prd_text();
    let idx = text.find("mock_justifications:").unwrap_or_else(|| {
        panic!(
            "PRD frontmatter has no 'mock_justifications:' field; agent/test-map.json \
             and agent/intent-card.json both cite it for AC8's deferral, so a dangling \
             reference is a broken paper trail, not a valid one"
        )
    });
    let rest = &text[idx..];
    let justification_block = &rest[..rest.find("\n- ").unwrap_or(rest.len())];

    assert!(
        justification_block.contains("AC8"),
        "mock_justifications does not name AC8 by id: {justification_block:?}"
    );
    assert!(
        justification_block.contains("burst")
            && (justification_block.contains("HCLOUD_TOKEN")
                || justification_block.contains("credential")
                || justification_block.contains("reachable")),
        "AC8's justification must name the concrete reason no ccx53 burst box was \
         reachable (credential/reachability), not a vague placeholder: \
         {justification_block:?}"
    );
}

#[test]
fn test_map_and_intent_card_ac8_entries_agree_with_the_frontmatter_deferral() {
    let test_map_path = repo_root().join("agent/test-map.json");
    let test_map = fs::read_to_string(&test_map_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", test_map_path.display()));
    assert!(
        test_map.contains("\"AC8\"") && test_map.contains("mock_justifications"),
        "agent/test-map.json's AC8 entry must point at mock_justifications now that the \
         PRD frontmatter actually carries that field"
    );

    let intent_card_path = repo_root().join("agent/intent-card.json");
    let intent_card = fs::read_to_string(&intent_card_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", intent_card_path.display()));
    assert!(
        intent_card.contains("mock_justifications"),
        "agent/intent-card.json's AC8 test field must point at mock_justifications now \
         that the PRD frontmatter actually carries that field"
    );
}
