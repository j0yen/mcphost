//! PRD-mcphost-oauth-resource-server
//! AC4 (P0) — Given a tool that requires a tenant, When it is called with
//! neither key nor bearer over streamable HTTP, Then the response is 401
//! with a `WWW-Authenticate` header naming the metadata URL; `signup`
//! without credentials still succeeds.
//!
//! PRD-mcphost-implicit-signup: the first Then clause no longer holds on
//! bare `/mcp` -- a `host.*` call with no credential at all now implicitly
//! signs up and runs as the new tenant instead of 401ing (requirement 4's
//! own "no longer reachable for host.*/billing.* on /mcp" -- see
//! tests/implsign_ac01_*.rs). The identical 401/WWW-Authenticate contract
//! for a real tenant's own `/t/{ns}/mcp` path is untouched (this PRD scopes
//! the implicit-signup branch to bare `/mcp` only) -- see
//! tests/tenantprm_ac04_401_challenge_names_tenant_or_root_metadata.rs's
//! `no_credential_on_tenant_path_names_the_tenant_metadata_url`, which
//! still passes unchanged.

use crate::common;
use common::{McpClient, TestServer, parse_response_body};
use serde_json::json;

async fn bare_call(client: &McpClient, name: &str, args: serde_json::Value) -> reqwest::Response {
    client
        .post_with_mcp_name_override(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": name, "arguments": args},
            }),
            name,
        )
        .await
}

#[tokio::test]
async fn no_credential_on_root_now_implicitly_signs_up_instead_of_401ing() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let resp = bare_call(&client, "host.state.get", json!({"key": "k"})).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "a bare host.* call on /mcp now succeeds");
    assert!(
        resp.headers().get(reqwest::header::WWW_AUTHENTICATE).is_none(),
        "a successful call must carry no WWW-Authenticate challenge"
    );

    // PRD-mcphost-implicit-signup: this call binds the session, upgrading
    // the response to text/event-stream -- see parse_response_body's doc.
    let body = parse_response_body(resp).await;
    assert!(body.get("error").is_none(), "must not error: {body:?}");
    let result = &body["result"];
    let structured = result
        .get("structuredContent")
        .cloned()
        .unwrap_or_else(|| result.clone());
    assert_eq!(structured["found"], json!(false), "a fresh implicit tenant has never set key 'k'");
    assert!(
        structured.get("onboarding").is_some(),
        "the call that created the implicit tenant must carry onboarding: {structured}"
    );
}

#[tokio::test]
async fn signup_without_credentials_still_succeeds() {
    let server = TestServer::start().await;
    let client = McpClient::new(&server.base_url);

    let resp = bare_call(&client, "signup", json!({"name": "No Credential Tenant"})).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    // PRD-mcphost-one-next-tool requirement 3: signup binds this session
    // and now emits notifications/tools/list_changed before its own
    // result, which upgrades this response from application/json to
    // text/event-stream (see common::parse_sse_messages's own doc) -- the
    // real JSON-RPC message is the last one on the wire either way.
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body: serde_json::Value = if content_type.starts_with("text/event-stream") {
        let text = resp.text().await.expect("read signup SSE body");
        common::parse_sse_messages(&text)
            .pop()
            .expect("signup SSE body carried no JSON-RPC message")
    } else {
        resp.json().await.expect("parse signup response")
    };
    assert!(body.get("error").is_none(), "signup must not error: {body:?}");
}
