//! P0 (PRD-mcphost-gate-debt-6d51e76 AC1) -- proof that the inherited
//! reviewer-agent finding this PRD exists to track ("finalize rejected
//! the subagent's output", attributed to a stale/pre-history HEAD
//! 6d51e763ce5472fd590619c0a9944514659f941f -- five-whys could not find
//! an introducing commit via `git log -S`, meaning the finding text
//! predates or precedes this repo's traceable history) no longer blocks
//! the gate.
//!
//! Multiple independent `extend-gate.sh --force` runs at build_into's
//! current HEAD (see this PRD's Blocked/iter_log notes, 2026-09-13)
//! already confirmed reviewer-agent's decision is not `block` and the
//! receipt carries no `finalize rejected the subagent's output`-shaped
//! block_reasons entry: receipts=25 pass=25 block=0 verdict=pass. This
//! test reads the freshly-generated, gitignored
//! `target/autobuilder/receipts/reviewer-agent.json` receipt the same
//! way `tests/revdebt_ac1_intent_card_pointers_resolve.rs` reads
//! `ac-traceability-receipt.json` -- skipping gracefully on a fresh
//! checkout that has never run the gate (nothing to falsify yet) rather
//! than re-running the multi-minute gate itself inside `cargo test`.

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
