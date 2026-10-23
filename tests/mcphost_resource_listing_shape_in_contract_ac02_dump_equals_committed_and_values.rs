//! AC2 -- PRD-mcphost-resource-listing-shape-in-contract. Given the
//! committed `contracts/host-tools.v1.json`, When the contract is dumped,
//! Then the bytes are identical and the `resources` values are the strings
//! `uri_for` and `list_entry` produce.

use mcphost::api_contract::{COMMITTED_CONTRACT, dump_contract_bytes};
use mcphost::kinds::KindRegistry;
use mcphost::resources::{ResourceKind, RESOURCE_TEMPLATES, list_entry, uri_for};
use serde_json::Value;

#[test]
fn mcphost_resource_listing_shape_in_contract_ac02_dump_equals_committed_byte_for_byte() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/host-tools.v1.json");
    let on_disk = std::fs::read(path).unwrap();
    let dumped = dump_contract_bytes(&KindRegistry::with_builtin());
    assert!(dumped == on_disk, "contracts/host-tools.v1.json is stale: run `mcphost contract dump`");
    assert_eq!(COMMITTED_CONTRACT.as_bytes(), dumped.as_slice());
}

#[test]
fn mcphost_resource_listing_shape_in_contract_ac02_values_are_what_the_host_lists() {
    let committed: Value = serde_json::from_str(COMMITTED_CONTRACT).unwrap();
    let r = &committed["resources"];
    assert_eq!(r["uri_scheme"]["table"], "mcphost://table/{name}");
    assert_eq!(r["uri_scheme"]["doc"], "mcphost://doc/{id}");
    assert_eq!(r["uri_scheme"]["run"], "mcphost://run/{id}/log");
    assert_eq!(r["name_field"]["tool"], "tool_spec:{name}");
    // Rendering each kind's placeholder through the listing's own
    // formatters reproduces the contract strings.
    for t in RESOURCE_TEMPLATES {
        let key = t.kind.contract_key();
        let placeholder = if matches!(t.kind, ResourceKind::Doc | ResourceKind::Run) { "{id}" } else { "{name}" };
        assert_eq!(r["uri_scheme"][key], uri_for(t.kind, placeholder).as_str(), "{key}");
        assert_eq!(r["name_field"][key], list_entry(t.kind, placeholder, None).name.as_str(), "{key}");
    }
}
