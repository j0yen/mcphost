//! PRD-mcphost-tenant-resource-metadata
//! AC7 (P0) — Given the existing key-based and OAuth test suites, When they
//! run against this build, Then they pass unchanged.
//!
//! This file is the tenantprm-prefixed proof point: signup -> publish ->
//! call -> admin over a key (no `/t/{ns}/mcp` anywhere), and a bearer JWT
//! against `/mcp` with an explicit, non-canonical `audience` -- the exact
//! shape `oauthrs_ac02_bearer_jwt_runs_as_tenant.rs`/
//! `oauthrs_ac03_invalid_tokens_rejected_with_reasons.rs`/
//! `oauthrs_ac10_key_based_flow_unchanged.rs` already pin.
//!
//! The rest of the proof is that none of those pre-existing files
//! (`oauthrs_ac*.rs`, `ac0*.rs`, `autherr_ac*.rs`) needed a single line
//! changed for this PRD -- `git diff main..HEAD -- tests/oauthrs_ac*.rs
//! tests/ac0*.rs tests/autherr_ac*.rs` is empty, and they still pass (see
//! this PRD's own build notes for the full suite run).

use crate::common;
use common::{McpClient, TestServer, publish, signup};
use serde_json::json;

use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1, sign};

#[tokio::test]
async fn key_based_signup_publish_call_and_whoami_still_work_on_mcp() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Unchanged Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let qualified = publish(
        &client,
        "hello",
        "echo",
        json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}}),
    )
    .await;
    assert_eq!(qualified, format!("{ns}.hello"));

    let result = client
        .tools_call(&qualified, json!({"msg": "hi"}))
        .await
        .expect("calling a key-authenticated tenant's own published tool must still work");
    assert_eq!(common::extract_structured(&result)["msg"], json!("hi"));

    let whoami = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect("host.whoami via key must still succeed");
    assert_eq!(common::extract_structured(&whoami)["tenant"], json!(ns));
}

#[tokio::test]
async fn bearer_jwt_with_an_explicit_custom_audience_still_runs_on_mcp() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "OAuth Unchanged Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://issuer.tenantprm-unchanged.example.com";
    let audience = "mcphost-test-audience";
    key_client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "audience": audience, "jwks_url": format!("{}/jwks", jwks.uri())}),
        )
        .await
        .expect("issuer_set with an explicit audience must still succeed");

    let token = sign(KID_1, priv_pem_1(), issuer, audience, "user-99", 300);
    let bearer_client = McpClient::with_bearer(&server.base_url, &token);
    let result = bearer_client
        .tools_call("host.state.set", json!({"key": "k", "value": "v"}))
        .await
        .expect("a bearer JWT with its own registered custom audience must still run on /mcp");
    let _ = common::extract_structured(&result);

    std::mem::forget(jwks);
}
