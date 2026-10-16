//! AC2 — Given `contracts/host-tools.v1.json` regenerated, When read, Then
//! `kinds.python.contract.error_families` lists `tool_code` with
//! `tool_exception` and `kinds.http.contract.source_position` is `none`.

use mcphost::api_contract::dump_contract;
use mcphost::kinds::KindRegistry;
use serde_json::Value;

fn assert_kind_contracts(doc: &Value) {
    let families = doc["kinds"]["python"]["contract"]["error_families"]
        .as_array()
        .expect("python lists error_families");
    let tool_code = families
        .iter()
        .find(|f| f["name"] == "tool_code")
        .expect("python lists the tool_code family");
    assert!(
        tool_code["codes"].as_array().unwrap().iter().any(|c| c == "tool_exception"),
        "tool_code lists tool_exception: {tool_code}"
    );
    assert_eq!(doc["kinds"]["http"]["contract"]["source_position"], "none");
    assert!(
        doc["kinds"]["python"]["contract"].get("fixtures").is_none(),
        "fixtures are omitted from the contract"
    );
}

#[test]
fn kconb_ac02_committed_contract_carries_kind_contracts() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/host-tools.v1.json");
    let committed: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_kind_contracts(&committed);
}

#[test]
fn kconb_ac02_fresh_dump_matches_committed_kind_contracts() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/host-tools.v1.json");
    let committed: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let fresh = dump_contract(&KindRegistry::with_builtin());
    assert_kind_contracts(&fresh);
    assert_eq!(fresh["kinds"], committed["kinds"], "contracts/host-tools.v1.json is stale: run `mcphost contract dump`");
}
