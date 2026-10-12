//! PRD-mcphost-data-quota-in-every-response
//! AC4 -- Given the `DATA_CEILINGS` set, When the test derives it from
//! `Plan::ceilings_flat()` keys matching `^(state|table|docs|export|ts)_` and
//! compares with the keys observed in AC 3, Then the sets are equal; Given a
//! test-only `Plan` field `state_fake_max`, Then the derived set gains it and
//! the observed set fails the test (the drift guard fires).

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use mcphost::plans::{PlanCatalog, data_ceilings_of};
use serde_json::json;
use std::collections::BTreeSet;

/// `^(state|table|docs|export|ts|blob|blobs)_`, written out independently
/// of the library's own prefix list. PRD-mcphost-blob-retention-and-quota R4
/// extends it with `blob`/`blobs` (`blobs_max`, `blob_bytes_max`,
/// `blob_max_bytes`) rather than adding a second drift test.
fn matches_prefix_regex(key: &str) -> bool {
    key.split_once('_')
        .is_some_and(|(head, _)| ["state", "table", "docs", "export", "ts", "blob", "blobs"].contains(&head))
}

fn derived(flat: &[(String, i64)]) -> BTreeSet<String> {
    flat.iter().map(|(k, _)| k.clone()).filter(|k| matches_prefix_regex(k)).collect()
}

fn keys(v: &serde_json::Value) -> BTreeSet<String> {
    v.as_object().expect("object").keys().cloned().collect()
}

/// One call of each data-verb family; the union of their `data.quota`.
async fn union_of_verb_quotas(client: &McpClient) -> serde_json::Value {
    let mut union = serde_json::Map::new();
    for tool in ["host.state.list", "host.table.list", "host.docs.status", "host.blob.list", "host.export"] {
        let body = extract_structured(&client.tools_call(tool, json!({})).await.unwrap());
        for (k, v) in body["data"]["quota"].as_object().unwrap_or_else(|| panic!("{tool}: {body:?}")) {
            union.insert(k.clone(), v.clone());
        }
    }
    serde_json::Value::Object(union)
}

#[tokio::test]
async fn observed_keys_equal_the_set_derived_from_the_plan_struct() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Quota").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let quickstart = extract_structured(&client.tools_call("host.quickstart", json!({"kind": "echo"})).await.unwrap());
    let usage = extract_structured(&client.tools_call("host.usage", json!({})).await.unwrap());
    let verbs = union_of_verb_quotas(&client).await;

    let mut observed = keys(&verbs);
    observed.extend(keys(&quickstart["limits"]["quota"]));
    observed.extend(keys(&usage["quota"]));

    let free_flat = server.state.plans.get("free").expect("free").ceilings_flat();
    let derived_set = derived(&free_flat);
    assert!(derived_set.len() >= 12, "the plan struct names at least nine data ceilings: {derived_set:?}");
    assert_eq!(observed, derived_set, "observed data.quota keys drifted from Plan::ceilings_flat");

    // The library's own generated set is the same derivation.
    let lib: BTreeSet<String> = data_ceilings_of(&free_flat).into_iter().map(|(k, _)| k).collect();
    assert_eq!(lib, derived_set);
    // Each surface alone carries every ceiling (no surface lags the others).
    assert_eq!(keys(&quickstart["limits"]["quota"]), derived_set);
    assert_eq!(keys(&usage["quota"]), derived_set);
    // Every plan in the catalog yields the same key set.
    for plan in &PlanCatalog::default_catalog().plans {
        assert_eq!(derived(&plan.ceilings_flat()), derived_set, "plan {}", plan.name);
    }
}

#[test]
fn a_new_plan_field_gains_a_derived_key_and_the_observed_set_fails_the_guard() {
    let plan = PlanCatalog::default_catalog().get("free").cloned().expect("free");
    let before = derived(&plan.ceilings_flat());
    // `state_fake_max` stands in for a field added to `Plan` (a test cannot
    // add a struct field, so it extends the flat view the struct yields).
    let mut flat = plan.ceilings_flat();
    flat.push(("state_fake_max".to_string(), 7));
    let after = derived(&flat);
    assert!(after.contains("state_fake_max"));
    assert_eq!(after.len(), before.len() + 1);
    // The library derivation gains it too -- no list to edit.
    assert!(data_ceilings_of(&flat).iter().any(|(k, _)| k == "state_fake_max"));
    // The surfaces as they render today do not carry it: the guard fires.
    let rendered = mcphost::plans::DataQuota::render(&plan, &Default::default());
    assert_ne!(keys(&rendered), after, "drift guard must fire when a key is missing from the surfaces");
}
