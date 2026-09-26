//! PRD-mcphost-oauth-resource-server
//! AC1 (P0) — Given no issuers registered, When `GET
//! /.well-known/oauth-protected-resource` is called, Then it returns 200
//! JSON with `resource` equal to the public URL.
//!
//! `authorization_servers` no longer reads `[]` here:
//! PRD-mcphost-hosted-authorization-server AC1 made mcphost always its own
//! authorization server, so with no tenant-registered bring-your-own issuer
//! this now lists exactly the host's own URL -- see
//! `hostedas_ac01_well_known_metadata.rs` for that PRD's own coverage of
//! this same endpoint.

use crate::common;
use common::TestServer;
use serde_json::Value;

#[tokio::test]
async fn empty_registry_returns_metadata_with_no_authorization_servers() {
    let server = TestServer::start().await;

    let resp = reqwest::get(format!("{}/.well-known/oauth-protected-resource", server.base_url))
        .await
        .expect("GET well-known");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: Value = resp.json().await.expect("parse metadata JSON");

    assert_eq!(body["resource"], Value::String(server.base_url.clone()));
    assert_eq!(body["authorization_servers"], serde_json::json!([server.base_url]));
    assert_eq!(body["bearer_methods_supported"], serde_json::json!(["header"]));
    assert_eq!(body["scopes_supported"], serde_json::json!(["mcp"]));
    assert!(
        body["resource_documentation"].as_str().is_some(),
        "resource_documentation must be present: {body}"
    );
}
