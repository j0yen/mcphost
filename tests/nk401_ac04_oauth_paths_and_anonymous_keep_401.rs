//! PRD-mcphost-named-key-refusal-not-http-401
//! AC4 -- ConnectionBound and PeerRecent refusals sent to `/t/{ns}/mcp` and
//! `/login/mcp`, plus an Anonymous `tenant_key_missing` and an
//! `invalid_token` on bare `/mcp`, are each HTTP 401 with the unchanged
//! `WWW-Authenticate: Bearer resource_metadata=..., scope="mcp"`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use mcphost::errors::Reason;
use mcphost::http::{PathClass, Upgrade, upgrade_for};
use serde_json::{Value, json};

fn call(name: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": {}}})
}

fn assert_401(resp: &reqwest::Response, what: &str) {
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED, "{what}");
    let challenge = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(challenge.starts_with("Bearer resource_metadata=\""), "{what}: {challenge}");
    assert!(challenge.ends_with("scope=\"mcp\""), "{what}: {challenge}");
}

#[tokio::test]
async fn nk401_ac04() {
    let server = TestServer::start().await;
    let base = &server.base_url;

    // Bare /mcp: Anonymous tenant_key_missing (admin.tenants never implicit-signs-up).
    let resp = McpClient::new(base).post_with_mcp_name_override(call("admin.tenants"), "admin.tenants").await;
    assert_401(&resp, "bare anonymous tenant_key_missing");

    // Bare /mcp: invalid_token.
    let resp = McpClient::with_bearer(base, "aaaa.bbbb.cccc")
        .post_with_mcp_name_override(call("host.whoami"), "host.whoami")
        .await;
    assert_401(&resp, "bare invalid_token");

    // A peer that redeemed a key (PeerRecent) and a connection that named a
    // tenant (ConnectionBound), both sent to the OAuth paths.
    let redeemer = McpClient::new(base);
    let signup = extract_structured(
        &redeemer.tools_call("signup", json!({"name": "R", "handoff": true})).await.expect("signup"),
    );
    let token = signup["handoff_token"].as_str().expect("token").to_string();
    let ns = signup["tenant"].as_str().expect("tenant").to_string();
    redeemer.tools_call("host.redeem", json!({"handoff_token": token})).await.expect("redeem");

    // `/login/mcp` is OAuth-only and `/t/{ns}/mcp` excludes the peer rule, so
    // the handler never raises ConnectionBound/PeerRecent there: the real
    // responses for the same keyless-after-redeem call keep the 401
    // challenge, and the decision for the two Reason cells is pinned by the
    // pure function the middleware applies.
    let resp = McpClient::new(base)
        .with_path("/login/mcp")
        .post_with_mcp_name_override(call("host.whoami"), "host.whoami")
        .await;
    assert_401(&resp, "/login/mcp keyless after redeem");
    let resp = McpClient::new(base)
        .with_path(&format!("/t/{ns}/mcp"))
        .post_with_mcp_name_override(call("host.whoami"), "host.whoami")
        .await;
    assert_401(&resp, "/t/{ns}/mcp keyless after redeem");
    for path in [PathClass::Login, PathClass::Tenant] {
        for reason in [Reason::ConnectionBound(ns.clone()), Reason::PeerRecent, Reason::Anonymous] {
            assert_eq!(upgrade_for(path, "tenant_key_missing", &reason), Upgrade::To401, "{path:?} {reason:?}");
        }
    }
}
