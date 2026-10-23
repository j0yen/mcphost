//! AC3 -- PRD-mcphost-resource-listing-shape-in-contract. Given a committed
//! contract whose `resources` template string differs from the registry's
//! (the fixture: a template changed), When `mcphost contract dump --check`
//! runs against it, Then it exits non-zero and names the `resources` key.

use std::process::Command;

use mcphost::api_contract::{COMMITTED_CONTRACT, drifted_top_level_keys, dump_contract};
use mcphost::kinds::KindRegistry;
use serde_json::{Value, json};

fn drifted_fixture() -> Value {
    let mut doc: Value = serde_json::from_str(COMMITTED_CONTRACT).unwrap();
    doc["resources"]["uri_scheme"]["table"] = json!("mcphost://tables/{name}");
    doc
}

#[test]
fn mcphost_resource_listing_shape_in_contract_ac03_drift_helper_names_resources_only() {
    let live = dump_contract(&KindRegistry::with_builtin());
    assert_eq!(drifted_top_level_keys(&drifted_fixture(), &live), vec!["resources".to_string()]);
    let committed: Value = serde_json::from_str(COMMITTED_CONTRACT).unwrap();
    assert!(drifted_top_level_keys(&committed, &live).is_empty());
}

#[test]
fn mcphost_resource_listing_shape_in_contract_ac03_check_exits_nonzero_naming_resources() {
    let dir = std::env::temp_dir().join(format!("mcphost-rlsic-ac03-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("host-tools.v1.json");
    let mut bytes = serde_json::to_string_pretty(&drifted_fixture()).unwrap();
    bytes.push('\n');
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["contract", "dump", "--check", "--path"])
        .arg(&path)
        .output()
        .expect("run mcphost contract dump --check");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!out.status.success(), "a drifted resources section must fail --check");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("resources"), "output must name the drifted key: {text}");
}
