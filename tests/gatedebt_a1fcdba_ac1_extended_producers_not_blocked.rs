//! P0 (PRD-mcphost-gate-debt-a1fcdba AC1) -- proof that the inherited
//! `extended-receipts` finding this PRD exists to track ("one or more
//! extended producers did not pass|skip") no longer blocks the gate at
//! HEAD a1fcdbad66bb40d3afe8f46ab118bf8d8aba0f3d or its descendants.
//!
//! Five-whys (commit 9e341b0, this PRD) found two producers blocking at
//! a1fcdba: `flake-audit` (a stale receipt one commit behind HEAD -- the
//! same commit that fixed the underlying ac01 test-pointer mismatch
//! already resolved it) and `cold-build-time` (a genuinely unconfigured
//! budget -- fixed by setting `cold_build_time_max_seconds=1500` in
//! `extended-gates.toml`). `scripts/extended-receipts.sh` (rustbuild
//! skill) rolls up every extended producer's own
//! `target/autobuilder/receipts/<name>-receipt.json` and fails closed
//! unless each one's `verdict` is `pass` or `skipped`. This test reads
//! those same two receipts -- the same way
//! `tests/gatedebt_6d51e76_ac1_reviewer_agent_not_blocked.rs` reads
//! `reviewer-agent.json` -- skipping gracefully on a fresh checkout that
//! has never run the gate (nothing to falsify yet) rather than
//! re-running the multi-minute gate itself inside `cargo test`.

use serde_json::Value;
use std::fs;
use std::path::Path;

/// Mirrors `extended-receipts.sh`'s own rollup rule: a producer blocks
/// the gate unless its receipt's `verdict` is exactly `pass` or `skipped`.
fn producer_blocks(receipt: &Value) -> bool {
    let verdict = receipt.get("verdict").and_then(Value::as_str).unwrap_or("");
    !matches!(verdict, "pass" | "skipped")
}

fn read_receipt(name: &str) -> Option<Value> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let receipt_path = manifest_dir.join(format!("target/autobuilder/receipts/{name}-receipt.json"));
    let text = match fs::read_to_string(&receipt_path) {
        Ok(t) => t,
        // Gitignored build artifact -- a fresh checkout that never ran
        // the gate has none yet, so there is nothing to falsify.
        Err(_) => return None,
    };
    Some(
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{receipt_path:?} must be valid JSON: {e}")),
    )
}

#[test]
fn flake_audit_does_not_block() {
    let Some(receipt) = read_receipt("flake-audit") else {
        return;
    };
    assert!(
        !producer_blocks(&receipt),
        "flake-audit must be pass|skipped, not the inherited a1fcdba block: {receipt:#?}"
    );
}

#[test]
fn cold_build_time_does_not_block() {
    let Some(receipt) = read_receipt("cold-build-time") else {
        return;
    };
    assert!(
        !producer_blocks(&receipt),
        "cold-build-time must be pass|skipped, not the inherited a1fcdba block: {receipt:#?}"
    );
}

#[test]
fn detects_a_blocking_producer_if_it_reappeared() {
    // Guards the helper above against being vacuously true: a receipt
    // carrying any verdict other than pass|skipped must be detected.
    let receipt: Value = serde_json::json!({"verdict": "block"});
    assert!(
        producer_blocks(&receipt),
        "a block-verdict receipt must be flagged as blocking"
    );
}

#[test]
fn a_passing_or_skipped_receipt_is_not_flagged() {
    assert!(!producer_blocks(&serde_json::json!({"verdict": "pass"})));
    assert!(!producer_blocks(&serde_json::json!({"verdict": "skipped"})));
}
