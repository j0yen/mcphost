//! PRD-mcphost-tenant-resource-metadata
//! AC6 (P0) — Given T, When `host.whoami` runs, Then the response carries
//! `resource == "<public>/t/acme/mcp"` and
//! `metadata_url == "<public>/.well-known/oauth-protected-resource/t/acme/mcp"`.

use crate::common;
use common::{McpClient, TestServer, signup};
use serde_json::json;

#[tokio::test]
async fn whoami_carries_canonical_resource_and_metadata_url() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Acme Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let result = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami must succeed");
    let whoami = common::extract_structured(&result);

    assert_eq!(whoami["resource"], json!(format!("{}/t/{ns}/mcp", server.base_url)));
    assert_eq!(
        whoami["metadata_url"],
        json!(format!("{}/.well-known/oauth-protected-resource/t/{ns}/mcp", server.base_url))
    );
}

#[tokio::test]
async fn whoami_over_the_tenant_path_reports_the_same_canonical_resource() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Acme Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key).with_path(&format!("/t/{ns}/mcp"));

    let result = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami over /t/{ns}/mcp must succeed");
    let whoami = common::extract_structured(&result);

    assert_eq!(whoami["resource"], json!(format!("{}/t/{ns}/mcp", server.base_url)));
}
