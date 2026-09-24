//! PRD-mcphost-admin-schema-contract
//! AC6 (P1) — Given a change to a listing field in `admin.rs` with no
//! schema change, When `cargo test` runs, Then the changelog-lint test
//! fails.
//!
//! Rather than diffing git history (fragile, and this repo's own convention
//! refuses tests keyed to "no changes since a sha" -- landing adds commits
//! that would self-invalidate such a test), this compares the LIVE field
//! set `admin.tenants`/`admin.usage` actually emit against the field set
//! `schemas/admin/*.v1.json` declares. A future edit that adds, removes, or
//! renames a listing field without a matching schema edit makes these two
//! sets diverge, and this test names exactly which field is out of sync --
//! the same failure AC2/AC3's `additionalProperties: false` validation
//! would eventually catch, but named explicitly and at both the row AND
//! top-level, rather than via a generic schema-validation error.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup};
use serde_json::Value;

fn crate_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn load_schema(rel: &str) -> Value {
    let path: PathBuf = crate_root().join(rel);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read schema {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse schema {}: {e}", path.display()))
}

fn object_fields(v: &Value) -> BTreeSet<String> {
    v.as_object()
        .unwrap_or_else(|| panic!("expected a JSON object, got {v:?}"))
        .keys()
        .cloned()
        .collect()
}

fn schema_properties(schema: &Value, pointer: &str) -> BTreeSet<String> {
    let node = schema
        .pointer(pointer)
        .unwrap_or_else(|| panic!("schema missing {pointer}: {schema:?}"));
    node.get("properties")
        .unwrap_or_else(|| panic!("schema node at {pointer} has no 'properties': {node:?}"))
        .as_object()
        .expect("properties object")
        .keys()
        .cloned()
        .collect()
}

fn assert_fields_match(what: &str, live: &BTreeSet<String>, schema: &BTreeSet<String>) {
    let live_only: Vec<&String> = live.difference(schema).collect();
    let schema_only: Vec<&String> = schema.difference(live).collect();
    assert!(
        live_only.is_empty() && schema_only.is_empty(),
        "{what}: admin.rs and the committed v1 schema disagree on field names -- \
         a listing field changed with no matching schema change. \
         Fields in admin.rs but not the schema: {live_only:?}. \
         Fields in the schema but not admin.rs: {schema_only:?}."
    );
}

#[tokio::test]
async fn admin_rs_listing_fields_match_the_committed_v1_schemas() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let (ns, key) = signup(&server.base_url, "Field Drift Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // At least one usage row, so the usage_row field-set check exercises a
    // real row rather than an empty array.
    client
        .tools_call(
            "host.tool_publish",
            serde_json::json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    client
        .tools_call(&format!("{ns}.hello"), serde_json::json!({}))
        .await
        .expect("call hello");

    let tenants_schema = load_schema("schemas/admin/tenants.v1.json");
    let tenants_result = admin
        .tools_call("admin.tenants", serde_json::json!({}))
        .await
        .expect("admin.tenants");
    let tenants_listing = extract_structured(&tenants_result);
    assert_fields_match(
        "admin.tenants top level",
        &object_fields(&tenants_listing),
        &schema_properties(&tenants_schema, ""),
    );
    let tenant_row = tenants_listing["tenants"]
        .as_array()
        .expect("tenants array")
        .first()
        .expect("at least one tenant row");
    assert_fields_match(
        "admin.tenants row",
        &object_fields(tenant_row),
        &schema_properties(&tenants_schema, "/$defs/tenant_row"),
    );

    let usage_schema = load_schema("schemas/admin/usage.v1.json");
    let usage_result = admin
        .tools_call("admin.usage", serde_json::json!({}))
        .await
        .expect("admin.usage");
    let usage_listing = extract_structured(&usage_result);
    assert_fields_match(
        "admin.usage top level",
        &object_fields(&usage_listing),
        &schema_properties(&usage_schema, ""),
    );
    let usage_row = usage_listing["usage"]
        .as_array()
        .expect("usage array")
        .first()
        .expect("at least one usage row");
    assert_fields_match(
        "admin.usage row",
        &object_fields(usage_row),
        &schema_properties(&usage_schema, "/$defs/usage_row"),
    );

    let last_prune = &usage_listing["last_prune"];
    if !last_prune.is_null() {
        assert_fields_match(
            "admin.usage last_prune",
            &object_fields(last_prune),
            &schema_properties(&usage_schema, "/properties/last_prune"),
        );
    }
}
