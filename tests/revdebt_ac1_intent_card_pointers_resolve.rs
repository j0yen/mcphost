//! P1 (PRD-mcphost-reviewer-debt-paydown Requirement 5 / AC6 / AC7) — a
//! meta-lane proof that reads `agent/intent-card.json` and fails, naming
//! the criterion and the path, when any acceptance criterion's `test`
//! field names a file that does not exist under `tests/`, or when the
//! card's PRD path and the traceability receipt's `prd_path` differ.
//!
//! Every PRD since 2026-09-11 shipped mcphost with a gate that reported
//! `delta-pass`, not `pass`, in part because the intent card's ten
//! acceptance-criteria `test` fields pointed at `tests/ac0N_*.rs` signup
//! tests that cover an unrelated feature, and the traceability receipt
//! keyed its criteria to a third PRD path neither the card's `prd_source`
//! nor its own `scope` agreed on. Nothing caught either drift because no
//! proof ever read the card's own pointers -- this file is that proof.
//! It is wired into the meta lane in `agent/proof-lanes.toml` (not only
//! the `rust-tests` lane's `tests/**/*.rs` glob) so that an edit which
//! only touches `agent/intent-card.json` -- exactly the shape of the
//! partial refresh that caused this debt -- still runs it.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// Checks that every acceptance criterion whose `test` field names a file
/// under `tests/` actually has that file on disk. Non-file `test` values
/// (a live-smoke note, a `deferred -- ...` note) are not path pointers and
/// are skipped -- only a leading `tests/...` token is checked, so a
/// trailing annotation like `tests/ac11_load_smoke.rs (#[ignore]d ...)`
/// still resolves against the file, not the whole string.
fn check_card_pointers(card: &Value, tests_dir: &Path) -> Result<(), String> {
    let acs = card
        .get("acceptance_criteria")
        .and_then(Value::as_array)
        .ok_or_else(|| "intent-card.json has no acceptance_criteria array".to_string())?;
    for ac in acs {
        let id = ac.get("id").and_then(Value::as_str).unwrap_or("<no id>");
        let test_field = ac.get("test").and_then(Value::as_str).unwrap_or("");
        let path_token = test_field.split_whitespace().next().unwrap_or("");
        if !path_token.starts_with("tests/") {
            continue;
        }
        let rel = path_token.trim_start_matches("tests/");
        let candidate: PathBuf = tests_dir.join(rel);
        if !candidate.is_file() {
            return Err(format!(
                "{id}'s test field names {path_token}, which does not exist under tests/"
            ));
        }
    }
    Ok(())
}

/// Checks that the card's own PRD path (`prd_source`) and the
/// traceability receipt's PRD path (`prd_path`) name the same PRD.
fn check_prd_path_consistency(card: &Value, receipt: &Value) -> Result<(), String> {
    let card_prd = card.get("prd_source").and_then(Value::as_str).unwrap_or("");
    let receipt_prd = receipt.get("prd_path").and_then(Value::as_str).unwrap_or("");
    if card_prd != receipt_prd {
        return Err(format!(
            "card prd_source {card_prd:?} differs from ac-traceability-receipt.json's prd_path {receipt_prd:?}"
        ));
    }
    Ok(())
}

#[test]
fn shipped_card_pointers_all_resolve() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let card_text = fs::read_to_string(manifest_dir.join("agent/intent-card.json"))
        .expect("agent/intent-card.json must exist");
    let card: Value =
        serde_json::from_str(&card_text).expect("intent-card.json must be valid JSON");
    let tests_dir = manifest_dir.join("tests");
    check_card_pointers(&card, &tests_dir)
        .expect("every AC test pointer in the shipped card must resolve to a real file under tests/");
}

#[test]
fn shipped_card_and_traceability_receipt_agree_on_one_prd_path() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let card_text = fs::read_to_string(manifest_dir.join("agent/intent-card.json"))
        .expect("agent/intent-card.json must exist");
    let card: Value = serde_json::from_str(&card_text).expect("intent-card.json must be valid JSON");
    let receipt_path = manifest_dir.join("target/autobuilder/receipts/ac-traceability-receipt.json");
    let receipt_text = match fs::read_to_string(&receipt_path) {
        Ok(t) => t,
        // The receipt is a regenerated, gitignored build artifact, not
        // committed -- a fresh checkout that never ran the gate has none
        // yet. Nothing to cross-check against; the other test in this
        // file still catches a stale test pointer on its own.
        Err(_) => return,
    };
    let receipt: Value =
        serde_json::from_str(&receipt_text).expect("ac-traceability-receipt.json must be valid JSON");
    check_prd_path_consistency(&card, &receipt)
        .expect("card prd_source and traceability receipt prd_path must name the same PRD");
}

#[test]
fn detects_a_stale_pointer_naming_the_criterion_and_path() {
    // AC6: a card whose AC7 `test` field names tests/does_not_exist.rs
    // must fail the proof, naming AC7 and that path.
    let card: Value = serde_json::json!({
        "acceptance_criteria": [
            {"id": "AC7", "test": "tests/does_not_exist.rs"}
        ]
    });
    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let err = check_card_pointers(&card, &tests_dir).expect_err("a missing test file must fail the proof");
    assert!(err.contains("AC7"), "error must name the criterion: {err}");
    assert!(err.contains("tests/does_not_exist.rs"), "error must name the path: {err}");
}

#[test]
fn an_existing_pointer_passes() {
    let card: Value = serde_json::json!({
        "acceptance_criteria": [
            {"id": "AC1", "test": "tests/revdebt_ac1_intent_card_pointers_resolve.rs"}
        ]
    });
    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    check_card_pointers(&card, &tests_dir).expect("this test's own file exists and must pass");
}

#[test]
fn a_non_file_test_annotation_is_not_treated_as_a_stale_pointer() {
    let card: Value = serde_json::json!({
        "acceptance_criteria": [
            {"id": "AC12", "test": "no unit test file; live smoke -- see target/autobuilder/ac12-preflight-smoke.json"},
            {"id": "AC19", "test": "deferred -- no test; see agent/intent-card.json acceptance_criteria AC19.test"}
        ]
    });
    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    check_card_pointers(&card, &tests_dir).expect("non tests/ annotations must not be treated as file pointers");
}

#[test]
fn detects_a_prd_path_mismatch_naming_both_paths() {
    // AC7: a card whose PRD path differs from the traceability receipt's
    // prd_path must fail, naming both paths.
    let card: Value = serde_json::json!({"prd_source": "/repo/PRD-a.md"});
    let receipt: Value = serde_json::json!({"prd_path": "/repo/PRD-b.md"});
    let err = check_prd_path_consistency(&card, &receipt).expect_err("a PRD-path mismatch must fail the proof");
    assert!(err.contains("/repo/PRD-a.md"), "error must name the card's path: {err}");
    assert!(err.contains("/repo/PRD-b.md"), "error must name the receipt's path: {err}");
}

#[test]
fn matching_prd_paths_pass() {
    let card: Value = serde_json::json!({"prd_source": "/repo/PRD-a.md"});
    let receipt: Value = serde_json::json!({"prd_path": "/repo/PRD-a.md"});
    check_prd_path_consistency(&card, &receipt).expect("identical prd paths must pass");
}
