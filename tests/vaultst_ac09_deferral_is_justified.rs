//! PRD-mcphost-upstream-token-vault-status AC9 (P0, Live) -- the prod-after-
//! deploy leg of AC9 (the operator hand-registering the operator tenant's
//! placeholder `slack` provider on mcphost-1, then reading `host.vault.status`
//! / `admin.vault.stats` over `https://mcphost.dev/mcp`) is not something
//! this coding sandbox can execute; provisioning that row and holding the
//! operator/admin keys are operator actions. The AC's own always-on
//! mechanism proof lives in `tests/vaultst_ac09_live_vault_status_trailer.rs`,
//! gated on `MCPHOST_LIVE=1` for the real leg. What this file proves is
//! that the deferral of the *prod* half was declared honestly, not
//! silently: the PRD frontmatter records `deferred_acs: [9]` and a
//! concrete `mock_justifications` entry naming AC9, and
//! `agent/test-map.json` / `agent/intent-card.json` agree with that
//! frontmatter instead of dangling a bare "deferred" string (the exact
//! defect `tests/checkcompat_race_ac08_deferral_is_justified.rs` and
//! `tests/statusfeed_ac11_deferral_is_justified.rs` already locked for
//! other PRDs).

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn prd_text() -> String {
    let path = repo_root().join("PRD-mcphost-upstream-token-vault-status.md");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn deferred_acs_frontmatter_names_ac9() {
    let text = prd_text();
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("- deferred_acs:"))
        .unwrap_or_else(|| {
            panic!(
                "PRD frontmatter has no '- deferred_acs:' line; AC9 is deferred (its prod \
                 leg needs the operator tenant's placeholder-credential slack provider \
                 hand-registered on mcphost-1 plus the admin key from orch) but that must \
                 be declared, not left implicit"
            )
        });
    assert!(
        line.contains('9'),
        "PRD frontmatter's deferred_acs line does not list AC9: {line:?}"
    );
}

#[test]
fn mock_justifications_names_ac9_with_a_concrete_reason() {
    let text = prd_text();
    let idx = text.find("mock_justifications:").unwrap_or_else(|| {
        panic!(
            "PRD frontmatter has no 'mock_justifications:' field; agent/test-map.json and \
             agent/intent-card.json both must cite it for AC9's deferral, so a dangling \
             reference is a broken paper trail, not a valid one"
        )
    });
    let rest = &text[idx..];
    let justification_block = &rest[..rest.find("\n- ").unwrap_or(rest.len())];

    assert!(
        justification_block.contains("AC9"),
        "mock_justifications does not name AC9 by id: {justification_block:?}"
    );
    assert!(
        justification_block.contains("mcphost-1") && justification_block.contains("operator"),
        "AC9's justification must name the concrete reason (the operator tenant's row on \
         mcphost-1, hand-registered by an operator), not a vague placeholder: \
         {justification_block:?}"
    );
    assert!(
        justification_block.contains("vaultst_ac09_live_vault_status_trailer.rs"),
        "AC9's justification must name the live test file that carries the always-on \
         mechanism proof: {justification_block:?}"
    );
    assert!(
        justification_block.contains("not counted as AC9's proof"),
        "AC9's justification must state the honest scope -- the always-on test proves the \
         branch's mechanism, not AC9 itself: {justification_block:?}"
    );
}

#[test]
fn test_map_and_intent_card_ac9_entries_agree_with_the_frontmatter_deferral() {
    let test_map_path = repo_root().join("agent/test-map.json");
    let test_map = fs::read_to_string(&test_map_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", test_map_path.display()));
    assert!(
        test_map.contains("\"AC9\"") && test_map.contains("mock_justifications"),
        "agent/test-map.json's AC9 entry must point at mock_justifications now that the PRD \
         frontmatter actually carries that field"
    );

    // agent/intent-card.json is refreshed on every wm-build run to name
    // whichever PRD is CURRENTLY building at HEAD (agent/test-map.json's
    // own ac_test_map_contract documents the identical pattern). This
    // assertion only holds while mcphost-upstream-token-vault-status is
    // still that PRD; a later PRD's legitimate refresh rewrites the card to
    // its own content and has no reason to preserve this string -- skip
    // with a printed reason rather than fail on expected drift (same fix
    // shape as tests/checkcompat_race_ac08_deferral_is_justified.rs's own
    // test).
    let intent_card_path = repo_root().join("agent/intent-card.json");
    let intent_card = fs::read_to_string(&intent_card_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", intent_card_path.display()));
    if !intent_card.contains("PRD-mcphost-upstream-token-vault-status") {
        eprintln!(
            "skip test_map_and_intent_card_ac9_entries_agree_with_the_frontmatter_deferral: \
             agent/intent-card.json no longer names PRD-mcphost-upstream-token-vault-status \
             -- a later PRD has refreshed the card since (expected drift), not a paper-trail \
             defect"
        );
        return;
    }
    assert!(
        intent_card.contains("mock_justifications"),
        "agent/intent-card.json's AC9 test field must point at mock_justifications now that \
         the PRD frontmatter actually carries that field"
    );
}
