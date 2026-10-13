//! PRD-mcphost-host-changelog-since AC2 — Given a tool present in
//! `host_tools` and absent from `contracts/host-tools.v1.json`, When the
//! contract-drift test runs, Then it fails naming the tool and the version it
//! must record as `since`.

use mcphost::api_contract::{COMMITTED_CONTRACT, since_drift};
use mcphost::kinds::KindRegistry;
use serde_json::Value;

#[test]
fn committed_contract_records_a_valid_since_for_every_live_tool() {
    let problems = since_drift(COMMITTED_CONTRACT, &KindRegistry::with_builtin());
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn a_live_tool_missing_from_the_committed_file_fails_naming_tool_and_version() {
    let mut v: Value = serde_json::from_str(COMMITTED_CONTRACT).unwrap();
    let tools = v["tools"].as_array_mut().unwrap();
    let at = tools.iter().position(|t| t["name"] == "host.changelog").expect("host.changelog in file");
    tools.remove(at);
    let problems = since_drift(&v.to_string(), &KindRegistry::with_builtin());
    let msg = problems.join("\n");
    assert!(msg.contains("host.changelog"), "{msg}");
    assert!(msg.contains(&format!("\"since\": \"{}\"", env!("CARGO_PKG_VERSION"))), "{msg}");
}

#[test]
fn a_since_newer_than_the_crate_version_fails() {
    let mut v: Value = serde_json::from_str(COMMITTED_CONTRACT).unwrap();
    v["tools"][0]["since"] = Value::String("999.0.0".into());
    let msg = since_drift(&v.to_string(), &KindRegistry::with_builtin()).join("\n");
    assert!(msg.contains("999.0.0") && msg.contains("newer"), "{msg}");
}
