//! PRD-mcphost-oauth-metadata-lists-hosted-as
//! AC1 (P0) — Given tenant T with no issuers and no provider, When `GET
//! /.well-known/oauth-protected-resource/t/<ns>/mcp` is called, Then
//! `authorization_servers == ["<public_url>"]` and `resource ==
//! "<public_url>/t/<ns>/mcp"`.

use crate::common;
use common::{TestServer, signup};
use serde_json::{Value, json};

#[tokio::test]
async fn tenant_without_issuers_or_provider_lists_the_hosted_as() {
    let server = TestServer::start().await;
    let (ns, _key) = signup(&server.base_url, "Bare Tenant").await;

    let resp = reqwest::get(format!("{}/.well-known/oauth-protected-resource/t/{ns}/mcp", server.base_url))
        .await
        .expect("GET tenant well-known");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: Value = resp.json().await.expect("parse tenant metadata JSON");

    assert_eq!(
        body["authorization_servers"],
        json!([server.base_url]),
        "a tenant with no issuer and no provider must still list the hosted AS: {body}"
    );
    assert_eq!(body["resource"], json!(format!("{}/t/{ns}/mcp", server.base_url)));
}
