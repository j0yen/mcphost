//! PRD-mcphost-paged-trait-on-every-list-verb
//! AC4 (P0) -- Given every contract entry, When this test runs over the
//! suite's recorded responses, Then every entry with a top-level array is
//! `paged: true` or carries `unpaged_reason`, and a new `json!({"items": [..]})`
//! handler without the derive fails the test naming the verb.

use crate::common;
use mcphost::paged;
use serde_json::{Value, json};

fn contract_tools() -> Vec<Value> {
    let kinds = mcphost::kinds::KindRegistry::with_builtin();
    mcphost::api_contract::dump_contract(&kinds)["tools"]
        .as_array()
        .expect("contract tools")
        .clone()
}

#[tokio::test]
async fn every_recorded_array_verb_is_paged_or_carries_an_unpaged_reason() {
    let tools = contract_tools();
    let recorded = common::pgtr_record_responses().await;
    assert!(
        recorded.iter().filter(|(_, r)| paged::has_top_level_array(r)).count() >= 30,
        "the recorder must reach the list verbs, got {}",
        recorded.len()
    );
    let violations = paged::drift_violations(&tools, &recorded);
    assert!(violations.is_empty(), "array-returning verbs with no paging verdict:\n{}", violations.join("\n"));
}

#[test]
fn a_new_items_handler_without_the_derive_fails_naming_its_verb() {
    let tools = contract_tools();
    let recorded = vec![("host.webhook.list".to_string(), json!({"items": [1, 2]}))];
    // The contract has no `host.webhook.list` entry at all: not paged, no reason.
    let violations = paged::drift_violations(&tools, &recorded);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].starts_with("host.webhook.list:"), "must name the verb: {violations:?}");

    // An entry that carries a too-short reason is still a violation.
    let mut with_entry = tools.clone();
    with_entry.push(json!({"name": "host.webhook.list", "unpaged_reason": "later"}));
    assert_eq!(paged::drift_violations(&with_entry, &recorded).len(), 1);

    // `paged: true`, or a real reason, clears it.
    with_entry.pop();
    with_entry.push(json!({"name": "host.webhook.list", "paged": true}));
    assert!(paged::drift_violations(&with_entry, &recorded).is_empty());
    with_entry.pop();
    with_entry.push(json!({"name": "host.webhook.list", "unpaged_reason": "bounded to five rows by design"}));
    assert!(paged::drift_violations(&with_entry, &recorded).is_empty());
}

#[test]
fn the_paged_set_and_the_excused_set_are_disjoint_and_generated_into_the_contract() {
    for (verb, reason) in paged::UNPAGED_VERBS {
        assert!(reason.chars().count() >= 20, "{verb}: unpaged_reason too short: {reason:?}");
        assert!(!paged::PAGED_VERBS.contains(verb), "{verb} is both paged and excused");
    }
    let tools = contract_tools();
    for verb in paged::PAGED_VERBS {
        let entry = tools.iter().find(|t| t["name"] == json!(verb)).unwrap_or_else(|| panic!("{verb} not in contract"));
        assert_eq!(entry["paged"], json!(true), "{verb}");
        assert!(entry.get("unpaged_reason").is_none(), "{verb}");
    }
}
