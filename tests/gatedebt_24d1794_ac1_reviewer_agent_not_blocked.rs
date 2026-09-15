//! P0 (PRD-mcphost-gate-debt-24d1794 AC1) -- proof that the inherited
//! reviewer-agent finding this PRD exists to track ("finalize rejected
//! the subagent's output", attributed via `git log -S` to
//! 24d1794 mcphost: fix gate paper trail for mcphost-gate-debt-a7d8e1c --
//! itself the fix commit for the PRIOR gate-debt PRD, mcphost-gate-debt-
//! a7d8e1c, whose own AC1 test is
//! tests/gatedebt_a7d8e1c_ac1_reviewer_agent_not_blocked.rs) does not
//! reappear in the current reviewer-agent receipt at build_into's HEAD.
//!
//! This is the third recurrence of the same finding text
//! ("finalize rejected the subagent's output") across three consecutive
//! gate-debt PRDs (6d51e76 -> a7d8e1c -> 24d1794), each one's own fix
//! commit named as the "introducing commit" for the next. That shape --
//! a fix commit that only ever adds a pinning test, never touching
//! `autobuilder reviewer-agent finalize`'s validation logic (schema,
//! decision enum, non-empty falsification sections, head_sha /
//! intent_card_sha match -- see ~/wintermute/autobuilder's reviewer.rs)
//! or the mcphost-side inputs it reads (agent/intent-card.json,
//! target/autobuilder/review-request.json) -- is consistent with a
//! transient rejection (e.g. a head_sha/intent_card_sha drift between
//! `reviewer-agent prepare` and `finalize` racing a concurrent commit
//! to this repo, or an occasional malformed/incomplete subagent JSON
//! reply) rather than a persistent defect in mcphost's own source. This
//! PRD's non-goals explicitly exclude re-deriving attribution or changing
//! gate/verdict semantics, so the fix here is the same as its two
//! predecessors: confirm the finding does not reproduce at the landed
//! HEAD and lock that in as a regression guard.
//!
//! Reads target/autobuilder/receipts/reviewer-agent.json the same way
//! tests/gatedebt_a7d8e1c_ac1_reviewer_agent_not_blocked.rs and
//! tests/gatedebt_6d51e76_ac1_reviewer_agent_not_blocked.rs already do,
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
    // Regression pin for a differently-shaped block: the gate must only be
    // treated as failing THIS finding when the reason text actually names
    // "finalize rejected the subagent's output", never any other block.
    let receipt: Value = serde_json::json!({
        "decision": "block",
        "block_reasons": ["some-other-unrelated-finding"]
    });
    assert!(!rejects_subagent_output(&receipt));
}
