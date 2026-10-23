//! AC1 -- PRD-mcphost-resource-listing-shape-in-contract. Given
//! `RESOURCE_TEMPLATES`, When the contract is dumped, Then the top-level
//! `resources` object carries `uri_scheme` and `name_field`, each keyed by
//! exactly the template kinds.

use std::collections::BTreeSet;

use mcphost::api_contract::dump_contract;
use mcphost::kinds::KindRegistry;
use mcphost::resources::{RESOURCE_TEMPLATES, contract_section};
use serde_json::Value;

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().expect("object").keys().cloned().collect()
}

#[test]
fn mcphost_resource_listing_shape_in_contract_ac01_key_set_equals_template_kinds() {
    let kinds: BTreeSet<String> = RESOURCE_TEMPLATES.iter().map(|t| t.kind.contract_key().to_string()).collect();
    assert_eq!(kinds.len(), RESOURCE_TEMPLATES.len(), "contract keys are distinct per kind");
    let dump = dump_contract(&KindRegistry::with_builtin());
    for doc in [&dump["resources"], &contract_section()] {
        assert_eq!(keys(&doc["uri_scheme"]), kinds);
        assert_eq!(keys(&doc["name_field"]), kinds);
    }
}

#[test]
fn mcphost_resource_listing_shape_in_contract_ac01_committed_file_carries_resources() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/host-tools.v1.json");
    let committed: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(committed["resources"]["uri_scheme"].is_object(), "run `mcphost contract dump`");
    assert!(committed["resources"]["name_field"].is_object());
}
