//! PRD-mcphost-admin-schema-contract
//! AC2 (P0) — Given a database with normal, disabled, self-offboarded, and
//! harness tenants, When both listings are validated against their v1
//! schema, Then every row validates.

use std::path::{Path, PathBuf};

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured, signup, signup_with_synthetic_header};
use serde_json::{Value, json};

fn crate_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn load_schema(rel: &str) -> Value {
    let path: PathBuf = crate_root().join(rel);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read schema {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse schema {}: {e}", path.display()))
}

#[tokio::test]
async fn every_row_of_both_listings_validates_against_v1_schema() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    // Normal tenant.
    let (normal_ns, normal_key) = signup(&server.base_url, "Normal Tenant").await;
    let normal_client = McpClient::with_bearer(&server.base_url, &normal_key);

    // Disabled tenant (admin-disabled, not self-offboarded).
    let (disabled_ns, _disabled_key) = signup(&server.base_url, "Disabled Tenant").await;
    admin
        .tools_call("admin.tenant_disable", json!({"tenant": disabled_ns}))
        .await
        .expect("admin.tenant_disable");

    // Self-offboarded tenant.
    let (offboard_ns, offboard_key) = signup(&server.base_url, "Self Offboard Tenant").await;
    let offboard_client = McpClient::with_bearer(&server.base_url, &offboard_key);
    offboard_client
        .tools_call("host.self_offboard", json!({}))
        .await
        .expect("host.self_offboard");

    // Harness-prefixed tenant (explicit x-mcphost-synthetic stamp).
    let harness_signup =
        signup_with_synthetic_header(&server.base_url, "Harness Tenant", "harness:probe").await;
    let harness_ns = harness_signup["tenant"]
        .as_str()
        .expect("harness tenant field")
        .to_string();

    // Generate at least one admin.usage row (and non-zero p50/p95) so usage
    // row validation exercises real data, not just an empty array.
    normal_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "hello", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    normal_client
        .tools_call(&format!("{normal_ns}.hello"), json!({}))
        .await
        .expect("call hello");

    let tenants_schema = load_schema("schemas/admin/tenants.v1.json");
    let tenants_validator =
        jsonschema::validator_for(&tenants_schema).expect("compile tenants.v1.json");
    let tenants_result = admin
        .tools_call("admin.tenants", json!({}))
        .await
        .expect("admin.tenants");
    let tenants_listing = extract_structured(&tenants_result);
    if let Some(err) = tenants_validator.iter_errors(&tenants_listing).next() {
        panic!("admin.tenants listing failed v1 schema: {err} at {}", err.instance_path);
    }
    let tenant_rows = tenants_listing["tenants"].as_array().expect("tenants array");
    for expect_ns in [&normal_ns, &disabled_ns, &offboard_ns, &harness_ns] {
        assert!(
            tenant_rows.iter().any(|r| r["tenant"] == json!(expect_ns)),
            "tenant {expect_ns} missing from admin.tenants listing: {tenant_rows:?}"
        );
    }

    let usage_schema = load_schema("schemas/admin/usage.v1.json");
    let usage_validator = jsonschema::validator_for(&usage_schema).expect("compile usage.v1.json");
    let usage_result = admin
        .tools_call("admin.usage", json!({}))
        .await
        .expect("admin.usage");
    let usage_listing = extract_structured(&usage_result);
    if let Some(err) = usage_validator.iter_errors(&usage_listing).next() {
        panic!("admin.usage listing failed v1 schema: {err} at {}", err.instance_path);
    }
    let usage_rows = usage_listing["usage"].as_array().expect("usage array");
    assert!(
        !usage_rows.is_empty(),
        "expected at least one admin.usage row from the hello call: {usage_listing:?}"
    );
}
