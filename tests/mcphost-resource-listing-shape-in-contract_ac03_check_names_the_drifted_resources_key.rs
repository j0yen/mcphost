//! PRD-mcphost-resource-listing-shape-in-contract
//! AC3 (P0) — Given a `RESOURCE_TEMPLATES` entry whose template string is
//! changed in a test fixture, When `mcphost contract dump --check` runs
//! against the unchanged committed file, Then it exits non-zero and its
//! output contains the drifted `resources` key.

use mcphost::resources::{RESOURCE_TEMPLATES, ResourceKind, ResourceTemplate, contract_drift};
use serde_json::Value;
use std::process::Command;

const COMMITTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json");

fn committed() -> Value {
    serde_json::from_slice(&std::fs::read(COMMITTED).unwrap()).unwrap()
}

#[test]
fn mcphost_resource_listing_shape_in_contract_ac03_a_changed_template_is_named_against_the_committed_file() {
    // The fixture registry: the real rows, with the table template moved.
    let fixture: Vec<ResourceTemplate> = RESOURCE_TEMPLATES
        .iter()
        .map(|t| ResourceTemplate {
            uri_template: if t.kind == ResourceKind::Table { "mcphost://tables/{name}" } else { t.uri_template },
            ..*t
        })
        .collect();
    assert_eq!(contract_drift(&committed(), RESOURCE_TEMPLATES), Vec::<String>::new(), "real registry: no drift");
    assert_eq!(contract_drift(&committed(), &fixture), vec!["resources.uri_scheme.table".to_string()]);
    assert_eq!(contract_drift(&serde_json::json!({}), RESOURCE_TEMPLATES), vec!["resources".to_string()]);
}

#[test]
fn mcphost_resource_listing_shape_in_contract_ac03_check_exits_nonzero_naming_the_drifted_key() {
    let dir = std::env::temp_dir().join(format!("mcphost-rlsic-ac03-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("host-tools.v1.json");
    let mut stale = committed();
    stale["resources"]["uri_scheme"]["table"] = Value::from("mcphost://tables/{name}");
    std::fs::write(&path, serde_json::to_string_pretty(&stale).unwrap() + "\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_mcphost"))
        .args(["contract", "dump", "--check", "--path"])
        .arg(&path)
        .output()
        .expect("run mcphost contract dump --check");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!out.status.success(), "a drifted resources section must fail --check");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("resources.uri_scheme.table"), "output names the drifted key: {text}");
}
