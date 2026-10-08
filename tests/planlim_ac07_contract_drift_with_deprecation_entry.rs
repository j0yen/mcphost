//! AC7 (PRD-mcphost-plan-limits-generated) — Given the contract dump, When
//! compared with the committed contract, Then the drift check passes with a
//! `contracts/deprecations.json` entry naming this PRD.
//!
//! The committed contract must equal a fresh dump; and the change this PRD
//! makes (a new `maximum` on `host.tool_call`'s budget fields) must be seen
//! by the comparator as a narrowing that the committed deprecation entry
//! excuses -- and that an empty ledger would NOT excuse.

use mcphost::api_contract::{Deprecation, diff, dump_contract, load_deprecations};
use mcphost::kinds::KindRegistry;
use serde_json::Value;
use std::path::Path;

const COMMITTED_CONTRACT: &[u8] = include_bytes!("../contracts/host-tools.v1.json");

/// The contract as it stood before this PRD: the committed one with every
/// `maximum` removed from each tool's `budget` properties.
fn contract_before_this_prd() -> Value {
    let mut old: Value = serde_json::from_slice(COMMITTED_CONTRACT).expect("committed contract");
    for tool in old["tools"].as_array_mut().unwrap() {
        let Some(props) = tool
            .get_mut("input_schema")
            .and_then(|s| s.get_mut("properties"))
            .and_then(|p| p.get_mut("budget"))
            .and_then(|b| b.get_mut("properties"))
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        for (_, field) in props.iter_mut() {
            field.as_object_mut().unwrap().remove("maximum");
        }
    }
    old
}

#[test]
fn committed_contract_matches_dump_and_deprecation_entry_excuses_the_new_maximum() {
    let live = dump_contract(&KindRegistry::with_builtin());
    let committed: Value = serde_json::from_slice(COMMITTED_CONTRACT).unwrap();
    assert_eq!(live, committed, "contracts/host-tools.v1.json is stale; run `mcphost contract dump`");

    let ledger = load_deprecations(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/deprecations.json")))
        .expect("deprecations.json parses");
    let entries: Vec<&Deprecation> = ledger
        .iter()
        .filter(|d| d.replacement.contains("PRD-mcphost-plan-limits-generated"))
        .collect();
    assert!(!entries.is_empty(), "a deprecations.json entry naming PRD-mcphost-plan-limits-generated");
    assert!(entries.iter().all(|d| d.lead_time_valid()));

    let old = contract_before_this_prd();
    let now = mcphost::state::now_unix();
    let unexcused = diff(&old, &live, &[], now);
    assert!(
        entries.iter().all(|e| unexcused.iter().any(|v| format!("{v:?}").contains(&e.path))),
        "without the entries the new maximum must be flagged at each path: {unexcused:?}"
    );
    let excused = diff(&old, &live, &ledger, now);
    assert!(excused.is_empty(), "with the ledger the drift check must pass: {excused:?}");
}
