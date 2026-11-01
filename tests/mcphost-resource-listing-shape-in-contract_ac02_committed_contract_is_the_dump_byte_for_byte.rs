//! PRD-mcphost-resource-listing-shape-in-contract
//! AC2 (P0) — Given the committed `contracts/host-tools.v1.json`, When
//! `mcphost contract dump` runs in a clean checkout, Then its output equals
//! the committed file byte for byte, `resources.uri_scheme.table` is
//! `mcphost://table/{name}`, `.doc` is `mcphost://doc/{id}` as `uri_for`
//! formats them, `.run` is `mcphost://run/{id}/log`, and
//! `resources.name_field.tool` is `tool_spec:{name}`.

use mcphost::api_contract::dump_contract_bytes;
use mcphost::kinds::KindRegistry;
use mcphost::resources::{ResourceKind, uri_for};
use serde_json::Value;

const COMMITTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/contracts/host-tools.v1.json");

#[test]
fn mcphost_resource_listing_shape_in_contract_ac02_committed_file_equals_the_dump_byte_for_byte() {
    let committed = std::fs::read(COMMITTED).expect("read committed contract");
    assert_eq!(committed, dump_contract_bytes(&KindRegistry::with_builtin()), "committed == mcphost contract dump");
}

#[test]
fn mcphost_resource_listing_shape_in_contract_ac02_uri_schemes_are_what_uri_for_formats() {
    let committed: Value = serde_json::from_slice(&std::fs::read(COMMITTED).unwrap()).unwrap();
    let scheme = &committed["resources"]["uri_scheme"];
    assert_eq!(scheme["table"], "mcphost://table/{name}");
    assert_eq!(scheme["doc"], "mcphost://doc/{id}");
    assert_eq!(scheme["run"], "mcphost://run/{id}/log");
    assert_eq!(scheme["tool"], "mcphost://tool/{name}/spec");
    assert_eq!(scheme["table"], uri_for(ResourceKind::Table, "{name}").as_str());
    assert_eq!(scheme["doc"], uri_for(ResourceKind::Doc, "{id}").as_str());
    assert_eq!(scheme["run"], uri_for(ResourceKind::Run, "{id}").as_str());
    assert_eq!(scheme["tool"], uri_for(ResourceKind::Tool, "{name}").as_str());
    assert_eq!(committed["resources"]["name_field"]["tool"], "tool_spec:{name}");
    assert_eq!(committed["resources"]["name_field"]["run"], "run_log:{id}");
}
