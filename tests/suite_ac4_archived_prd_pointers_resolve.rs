//! PRD-mcphost-test-suite-consolidation
//! AC4 (P0) -- Given the archived mcphost PRDs' test pointers (for example
//! `tests/hooks_ac1_*.rs`), When the verified-completed pairing classifier
//! runs against them, Then every pairing that resolved before resolves now.
//!
//! `revdebt_ac1_intent_card_pointers_resolve.rs` (PRD-mcphost-reviewer-debt-
//! paydown) already proves every `agent/intent-card.json` acceptance
//! criterion's `test` pointer resolves to an existing file. What THIS PRD
//! adds as a new risk is a second failure mode that check does not cover: a
//! file could still exist on disk (so the file-existence check passes) while
//! no longer being compiled into ANY suite binary (a `gen-test-suites.sh`
//! bucketing bug, or a stray suite file left over from a bad regen) -- the
//! pairing would "resolve" to a path but the test behind it would never run.
//! This file closes that gap: every `tests/...` pointer in the current
//! intent card must both exist AND be `#[path]`-included by exactly one
//! `tests/suite_*.rs` file.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn suite_files(tests_dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(tests_dir)
        .expect("read tests/")
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            name.starts_with("suite_core_") || name.starts_with("suite_sandbox_")
        })
        .collect()
}

/// How many of the generated suite files `#[path]`-include `basename`.
fn inclusion_count(suites: &[PathBuf], basename: &str) -> usize {
    let needle = format!(r#"#[path = "{basename}"]"#);
    suites
        .iter()
        .filter(|p| {
            fs::read_to_string(p)
                .map(|c| c.contains(&needle))
                .unwrap_or(false)
        })
        .count()
}

#[test]
fn intent_card_test_pointers_are_compiled_into_exactly_one_suite() {
    let root = repo_root();
    let tests_dir = root.join("tests");
    let card_path = root.join("agent/intent-card.json");
    let card_text = fs::read_to_string(&card_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", card_path.display()));
    let card: Value =
        serde_json::from_str(&card_text).expect("agent/intent-card.json is valid JSON");

    let acs = card
        .get("acceptance_criteria")
        .and_then(Value::as_array)
        .expect("intent-card.json has an acceptance_criteria array");

    let suites = suite_files(&tests_dir);
    assert!(
        !suites.is_empty(),
        "no tests/suite_*.rs files found -- has scripts/gen-test-suites.sh been run?"
    );

    let mut checked = 0usize;
    let mut offenders: Vec<String> = Vec::new();
    for ac in acs {
        let id = ac.get("id").and_then(Value::as_str).unwrap_or("<no id>");
        let test_field = ac.get("test").and_then(Value::as_str).unwrap_or("");
        let path_token = test_field.split_whitespace().next().unwrap_or("");
        let Some(rel) = path_token.strip_prefix("tests/") else {
            continue; // not a file pointer (a smoke note, a "deferred" note)
        };
        let basename = rel.rsplit('/').next().unwrap_or(rel);
        if !basename.ends_with(".rs") {
            continue;
        }
        checked += 1;

        if !tests_dir.join(rel).is_file() {
            offenders.push(format!("{id}: {path_token} does not exist on disk"));
            continue;
        }
        let count = inclusion_count(&suites, basename);
        if count == 0 {
            offenders.push(format!(
                "{id}: {path_token} exists but is not `#[path]`-included by any tests/suite_*.rs"
            ));
        } else if count > 1 {
            offenders.push(format!(
                "{id}: {path_token} is included by {count} suites -- must be exactly one"
            ));
        }
    }

    assert!(
        checked > 0,
        "no tests/... file pointers found in agent/intent-card.json's \
         acceptance_criteria -- this test would be vacuous"
    );
    assert!(
        offenders.is_empty(),
        "intent-card.json pointer(s) no longer resolve to a compiled test after \
         consolidation:\n{}",
        offenders.join("\n")
    );
}

/// `intent_card_test_pointers_are_compiled_into_exactly_one_suite` above only
/// checks that an AC's `test` pointer exists on disk and is compiled into one
/// suite binary -- it says nothing about whether that file actually proves
/// the AC it is attached to. A stale pointer left over from a prior PRD's
/// intent-card refresh (this card's own `ambiguities_resolved` log records
/// that recurring six times) can name a real, passing, compiled test that
/// proves a completely different feature. Every `tests/suite_ac<N>_*.rs`
/// proof file in this repo opens with a doc comment naming the PRD slug and
/// AC id it proves (see the top of this very file); this test cross-checks
/// each AC's `test` pointer against that header instead of trusting the
/// path alone.
#[test]
fn ac_test_pointer_target_names_its_own_prd_and_ac() {
    let root = repo_root();
    let tests_dir = root.join("tests");
    let card_path = root.join("agent/intent-card.json");
    let card_text = fs::read_to_string(&card_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", card_path.display()));
    let card: Value =
        serde_json::from_str(&card_text).expect("agent/intent-card.json is valid JSON");

    let prd_slug = card
        .get("intent_slug")
        .and_then(Value::as_str)
        .expect("intent-card.json has an intent_slug")
        .to_string();

    let acs = card
        .get("acceptance_criteria")
        .and_then(Value::as_array)
        .expect("intent-card.json has an acceptance_criteria array");

    let mut checked = 0usize;
    let mut mismatched: Vec<String> = Vec::new();
    for ac in acs {
        let id = ac.get("id").and_then(Value::as_str).unwrap_or("<no id>");
        let test_field = ac.get("test").and_then(Value::as_str).unwrap_or("");
        let Some(rel) = test_field
            .split_whitespace()
            .next()
            .and_then(|p| p.strip_prefix("tests/"))
        else {
            continue; // not a file pointer (a smoke note, a "deferred" note)
        };
        let path = tests_dir.join(rel);
        let Ok(contents) = fs::read_to_string(&path) else {
            continue; // covered by the file-existence check above
        };
        checked += 1;

        let bare_id = id.trim_start_matches("AC");
        let names_this_prd = contents.contains(&prd_slug);
        let names_this_ac = contents.contains(&format!("AC{bare_id} "))
            || contents.contains(&format!("AC{bare_id}("));
        if !(names_this_prd && names_this_ac) {
            mismatched.push(format!(
                "{id} -> {test_field} (does not name {prd_slug}/{id} in its own header)"
            ));
        }
    }

    assert!(
        checked > 0,
        "no tests/... file pointers found in agent/intent-card.json's \
         acceptance_criteria -- this test would be vacuous"
    );
    assert!(
        mismatched.is_empty(),
        "intent-card.json AC test pointers do not match their own PRD/AC in \
         the target file's header:\n{}",
        mismatched.join("\n")
    );
}
