//! contract: reads registry handler::host_tool_descriptors
//! contract: reads contracts/host-tools.v1.json
//! PRD-mcphost-output-schema-reach-sweep-live
//! AC1 (P0) -- Given every name `tools/list` serves, When the osreach sweep
//! drives each through real dispatch on a fresh tenant, with `dry_run: true`
//! where the input schema declares it, and after one live call of each
//! create-kind write verb, Then every success `structuredContent` validates
//! (jsonschema, draft 2020-12) against the schema as served, the exercised set
//! equals `contracts/host-tools.v1.json`, the test source has no tool-name
//! literal, and a failing result is reported with tool, state, instance path
//! and failing keyword.

use std::collections::BTreeSet;

use crate::osreach;
use serde_json::{Value, json};

#[tokio::test]
async fn osreach_ac01_every_success_validates_against_the_served_schema() {
    let sweep = osreach::sweep().await;
    for line in sweep.lines() {
        println!("{line}");
    }
    println!("osreach unreached={}", sweep.unreached_count());
    let failures = sweep.failures();
    assert!(failures.is_empty(), "{} result(s) the served schema rejects:\n{}", failures.len(), failures.join("\n"));
}

#[tokio::test]
async fn osreach_ac01_exercised_set_equals_the_committed_contract() {
    let sweep = osreach::sweep().await;
    let exercised: BTreeSet<String> = sweep.rows.keys().cloned().collect();
    let contract: Value = serde_json::from_str(
        &std::fs::read_to_string(osreach::root().join("contracts/host-tools.v1.json")).expect("contract"),
    )
    .expect("contract json");
    let committed: BTreeSet<String> =
        contract["tools"].as_array().expect("tools").iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect();
    let missing: Vec<&String> = committed.difference(&exercised).collect();
    let extra: Vec<&String> = exercised.difference(&committed).collect();
    assert!(missing.is_empty() && extra.is_empty(), "in the contract, not exercised: {missing:?}; exercised, not in the contract: {extra:?}");
    assert_eq!(sweep.rows.len(), sweep.served.len(), "every served name got a row");
    println!("osreach exercised {} served names over {} contract tools", sweep.rows.len(), committed.len());
}

#[tokio::test]
async fn osreach_ac01_every_state_is_driven() {
    let sweep = osreach::sweep().await;
    for s in &sweep.served {
        let row = &sweep.rows[&s.name];
        let states = 2 + usize::from(s.declares_dry_run());
        assert_eq!(row.calls, states, "{}: one call per state it supports", s.name);
    }
}

#[tokio::test]
async fn osreach_ac01_no_tool_name_literal_in_the_test_sources() {
    let sweep = osreach::sweep().await;
    for file in ["tests/osreach_ac01_sweep_validates_every_served_name.rs", "tests/support/osreach.rs"] {
        let src = std::fs::read_to_string(osreach::root().join(file)).expect("source");
        for s in &sweep.served {
            let quoted = format!("\"{}\"", s.name);
            assert!(!src.contains(&quoted), "{file} spells the tool name {quoted}");
        }
    }
}

#[test]
fn osreach_ac01_a_failing_result_names_tool_state_path_and_keyword() {
    let schema = json!({"type": "object", "properties": {"n": {"type": "integer"}}, "required": ["n"]});
    let msg = osreach::check("some.tool", "dry_run", &schema, &json!({"n": "text"})).expect_err("rejected");
    for part in ["some.tool", "dry_run", "instance_path=/n", "keyword=type"] {
        assert!(msg.contains(part), "{part} missing from {msg}");
    }
    assert!(osreach::check("some.tool", "fresh", &schema, &json!({"n": 1})).is_ok());
}
