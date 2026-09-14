//! P0 (PRD-mcphost-gate-debt-a7d8e1c AC1) -- proof that the inherited
//! reviewer-agent finding this PRD exists to track ("finalize rejected
//! the subagent's output", attributed via `git log -S` to
//! 5511083 mcphost: add AC1 test for mcphost-gate-debt-6d51e76 (fix
//! ac-number-collision) -- the commit that introduced the prior gate-debt
//! PRD's own regression test string, five-whys could not trace an earlier
//! introducing commit) does not reappear in the current reviewer-agent
//! receipt at build_into's HEAD.
//!
//! This is the same finding text mcphost-gate-debt-6d51e76's own AC1 test
//! (tests/gatedebt_6d51e76_ac1_reviewer_agent_not_blocked.rs) already
//! guards; PRD-build-gate-debt-auto-prd's attribution split re-surfaced it
//! against a newer HEAD (a7d8e1c) when this PRD was drafted. Reading
//! target/autobuilder/receipts/reviewer-agent.json at gate time (2026-09-14)
//! confirms `decision: "block"` with `block_reasons:
//! ["intent-card-refresh-commit-not-revert-clean"]` -- a different, later
//! finding out of this PRD's non-goals ("does not change gate/verdict
//! semantics or re-derive attribution") -- with no
//! "finalize rejected the subagent's output"-shaped entry anywhere in that
//! list. This test locks that in as a regression guard the same way
//! tests/gatedebt_6d51e76_ac1_reviewer_agent_not_blocked.rs already does,
//! skipping gracefully on a fresh checkout that has never run the gate
//! (nothing to falsify yet) rather than re-running the multi-minute gate
//! itself inside `cargo test`.

use serde_json::Value;
use std::fs;
use std::path::Path;

/// Returns true iff `receipt`'s decision is `block` or its `block_reasons`
/// contains an entry mentioning the named inherited finding.
fn rejects_subagent_output(receipt: &Value) -> bool {
    let decision = receipt.get("decision").and_then(Value::as_str).unwrap_or("");
    if decision == "block" {
        let block_reasons = receipt
            .get("block_reasons")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        return block_reasons.iter().any(|r| {
            r.to_string()
                .to_lowercase()
                .contains("finalize rejected the subagent")
        });
    }
    false
}

#[test]
fn reviewer_agent_does_not_reject_the_subagent_output() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let receipt_path = manifest_dir.join("target/autobuilder/receipts/reviewer-agent.json");
    let text = match fs::read_to_string(&receipt_path) {
        Ok(t) => t,
        // Gitignored build artifact -- a fresh checkout that never ran
        // the gate has none yet, so there is nothing to falsify.
        Err(_) => return,
    };
    let receipt: Value = serde_json::from_str(&text)
        .expect("target/autobuilder/receipts/reviewer-agent.json must be valid JSON");
    assert!(
        !rejects_subagent_output(&receipt),
        "the inherited finding this PRD exists to track ('finalize rejected \
         the subagent's output') must not reappear in the current reviewer-agent \
         receipt: {receipt:#?}"
    );
}

#[test]
fn detects_the_named_finding_if_it_reappeared() {
    // Guards the helper above against being vacuously true: a receipt
    // that DOES carry the named block reason must be detected.
    let receipt: Value = serde_json::json!({
        "decision": "block",
        "block_reasons": [
            {"id": "reviewer-agent", "note": "finalize rejected the subagent's output"}
        ]
    });
    assert!(
        rejects_subagent_output(&receipt),
        "fixture carrying the named finding must be detected as a rejection"
    );
}

#[test]
fn a_passing_receipt_is_not_flagged() {
    let receipt: Value = serde_json::json!({
        "decision": "pass",
        "block_reasons": []
    });
    assert!(!rejects_subagent_output(&receipt));
}

#[test]
fn a_block_for_an_unrelated_reason_is_not_flagged() {
    // Regression pin for the actual current shape: block=true but for a
    // different, out-of-scope finding (intent-card-refresh-commit-not-
    // revert-clean), never the named subagent-output rejection.
    let receipt: Value = serde_json::json!({
        "decision": "block",
        "block_reasons": ["intent-card-refresh-commit-not-revert-clean"]
    });
    assert!(!rejects_subagent_output(&receipt));
}
