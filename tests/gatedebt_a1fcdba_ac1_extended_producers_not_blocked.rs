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
//!
//! Head-sha freshness guard (mcphost-gate-debt-4f1112d): reviewer-agent's
//! 2026-09-15 concern on this file ("ac1-proof-test-vacuous-without-
//! headsha-check") named half of a staleness bug -- a stale PASS receipt
//! could mask a real regression, since `read_receipt` never compared
//! `receipt.head_sha` to the commit under test. The other half is worse
//! and is what this PRD's `extended-receipts` finding traced to: a stale
//! BLOCK receipt is self-perpetuating. `flake-audit`'s own producer
//! (`autobuilder-extended-gates`) runs `cargo test --quiet` K times and
//! only writes its OWN fresh receipt after all K finish -- so every one
//! of those K inner `cargo test` invocations, for the entire duration of
//! that run, sees the PREVIOUS run's receipt still on disk. Once that
//! receipt is ever `block` (a single flaky/contended sub-run is enough),
//! every inner `cargo test` in the NEXT flake-audit run fails this exact
//! test before flake-audit can ever produce a clean run of its own --
//! a fixed point that never self-heals without a human deleting the
//! receipt (which this repo's own conventions forbid doing to force a
//! pass). Comparing `receipt.head_sha` against the actual current HEAD
//! and treating a mismatch as "no evidence for HEAD yet" (same as a
//! missing receipt) breaks the lock: a receipt from any OTHER commit --
//! stale-pass or stale-block alike -- proves nothing about the commit
//! under test right now. `current_head_sha()` returning `None` (git
//! unavailable) fails OPEN, trusting the receipt as-is, rather than
//! silently masking a block a git-less environment has no way to refute.

use serde_json::Value;
use std::fs;
use std::path::Path;

/// Mirrors `extended-receipts.sh`'s own rollup rule: a producer blocks
/// the gate unless its receipt's `verdict` is exactly `pass` or `skipped`.
fn producer_blocks(receipt: &Value) -> bool {
    let verdict = receipt.get("verdict").and_then(Value::as_str).unwrap_or("");
    !matches!(verdict, "pass" | "skipped")
}

/// Current git HEAD sha, resolved from this crate's own checkout.
/// `None` on any failure (not a git repo, `git` missing, ...) -- callers
/// must fail OPEN on that (trust the receipt) rather than treat "can't
/// tell" as "definitely stale".
fn current_head_sha() -> Option<String> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().to_owned())
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
    let receipt: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{receipt_path:?} must be valid JSON: {e}"));
    // Freshness guard: see the module doc comment. A receipt naming a
    // DIFFERENT head_sha than the commit actually under test right now
    // proves nothing about it -- pass or block alike -- so it is treated
    // exactly like a missing receipt (nothing to falsify yet).
    if let Some(current) = current_head_sha() {
        let receipt_head = receipt.get("head_sha").and_then(Value::as_str).unwrap_or("");
        if receipt_head != current {
            return None;
        }
    }
    Some(receipt)
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

/// Regression pin for the self-perpetuating-lock bug this PRD fixes: a
/// `block` receipt left over from a DIFFERENT (older) HEAD must not fail
/// this test, or `flake-audit`'s own next run could never produce a clean
/// receipt of its own (see the module doc comment). Writes a real receipt
/// under `target/autobuilder/receipts/` at a name no real producer uses,
/// backs up nothing (the name is test-owned), and cleans up after itself.
#[test]
fn a_block_receipt_from_a_different_head_is_not_flagged() {
    let Some(current) = current_head_sha() else {
        // No git available in this environment -- read_receipt() fails
        // open (trusts the receipt) when it can't resolve HEAD, so there
        // is nothing this test can prove here either; skip rather than
        // assert on a path this function itself will not take.
        return;
    };
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let receipts_dir = manifest_dir.join("target/autobuilder/receipts");
    fs::create_dir_all(&receipts_dir).expect("create receipts dir");
    let receipt_path = receipts_dir.join("gatedebt-4f1112d-staleness-fixture-receipt.json");
    let stale_head = format!("{}f", &current[..current.len() - 1]); // guaranteed != current
    fs::write(
        &receipt_path,
        serde_json::json!({"verdict": "block", "head_sha": stale_head}).to_string(),
    )
    .expect("write fixture receipt");

    let result = read_receipt("gatedebt-4f1112d-staleness-fixture");

    let _ = fs::remove_file(&receipt_path);

    assert!(
        result.is_none(),
        "a receipt naming a head_sha other than current HEAD must be treated as no \
         evidence yet (None), not trusted as a current block: {result:#?}"
    );
}

/// Companion pin: a receipt naming the ACTUAL current HEAD is still read
/// and still enforced -- the freshness guard narrows what counts as
/// evidence, it does not disable the check entirely.
#[test]
fn a_receipt_for_the_current_head_is_still_read() {
    let Some(current) = current_head_sha() else {
        return;
    };
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let receipts_dir = manifest_dir.join("target/autobuilder/receipts");
    fs::create_dir_all(&receipts_dir).expect("create receipts dir");
    let receipt_path = receipts_dir.join("gatedebt-4f1112d-freshfixture-receipt.json");
    fs::write(
        &receipt_path,
        serde_json::json!({"verdict": "block", "head_sha": current}).to_string(),
    )
    .expect("write fixture receipt");

    let result = read_receipt("gatedebt-4f1112d-freshfixture");

    let _ = fs::remove_file(&receipt_path);

    assert!(
        result.is_some(),
        "a receipt naming the actual current HEAD must still be read as live evidence"
    );
}
