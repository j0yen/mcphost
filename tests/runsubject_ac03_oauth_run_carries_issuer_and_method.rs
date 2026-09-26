//! PRD-mcphost-runs-end-user-subject
//! AC3 (P0) — Given an OAuth-identified call, When its run is read, Then
//! `end_user.issuer` is the token issuer and `method == "oauth"`.

use crate::common;
use crate::oauth;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1, sign};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn oauth_identified_sync_call_run_carries_issuer_and_method() {
    let upstream = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ping"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&upstream)
        .await;

    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "AC3 Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": format!("{}/ping", upstream.uri()),
        "args_schema": {"type": "object"},
    });
    key_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ping_tool", "kind": "http", "spec": spec}),
        )
        .await
        .expect("publish ok");

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://issuer.example.com";
    let audience = "mcphost-test-audience";
    key_client
        .tools_call(
            "host.oauth.issuer_set",
            json!({
                "issuer": issuer,
                "audience": audience,
                "jwks_url": format!("{}/jwks", jwks.uri()),
            }),
        )
        .await
        .expect("issuer_set must succeed");

    let token = sign(KID_1, priv_pem_1(), issuer, audience, "u_oauth3", 300);
    let bearer_client = McpClient::with_bearer(&server.base_url, &token);

    let result = bearer_client
        .tools_call(&format!("{ns}.ping_tool"), json!({}))
        .await
        .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    assert_eq!(structured["status"], 200, "{structured}");

    let listed = extract_structured(
        &key_client
            .tools_call("host.runs.list", json!({"end_user_subject": "u_oauth3"}))
            .await
            .expect("runs.list ok"),
    );
    let runs = listed["runs"].as_array().expect("runs array");
    assert_eq!(runs.len(), 1, "expected exactly 1 run for u_oauth3: {listed}");
    assert_eq!(
        runs[0]["end_user"],
        json!({"subject": "u_oauth3", "issuer": issuer, "method": "oauth"}),
        "run's end_user: {}",
        runs[0]
    );
}
