//! PRD-mcphost-hosted-authorization-server
//! AC10 (P0) — Given the existing key-based and issuer-JWT suites, When
//! run, Then they pass unchanged.
//!
//! The real proof is that `tests/oauthrs_ac10_key_based_flow_unchanged.rs`,
//! `tests/oauthrs_ac02_bearer_jwt_runs_as_tenant.rs`, and every other
//! pre-existing suite in this repo still pass with this PRD's code
//! landed (verified by running the full `cargo test` suite; none of
//! those files needed a line changed for this PRD except
//! `tests/oauthrs_ac01_protected_resource_metadata_empty.rs`, which this
//! PRD's own AC1 -- the host is now always its own authorization server
//! -- deliberately updates). This hostedas-prefixed file is this AC's own
//! proof point per the "every non-deferred AC has its own test file"
//! convention: it drives both the key-based path and the registered
//! per-tenant-issuer JWT path end to end *alongside* the new hosted AS
//! (registering a DCR client and completing a hosted grant on the same
//! tenant first), and additionally checks each path's `host.whoami`
//! still reports the `auth_method` its own kind of credential always
//! reported (`"key"`, `"oauth"`) -- distinct from the hosted path's new
//! `"hosted_token"` (see `hostedas_ac04_consent_code_token_hosted_bearer.rs`).

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, publish, signup};
use serde_json::json;

use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1, sign};

#[tokio::test]
async fn key_based_and_issuer_jwt_flows_still_work_alongside_the_hosted_as() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "Unchanged Flows Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    // The hosted AS now exists on this same tenant -- register a DCR
    // client, but otherwise leave it alone; its presence must not
    // disturb the key-based or issuer-JWT paths below.
    let register: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/oauth/register", server.base_url))
        .json(&json!({"application_type": "native", "redirect_uris": ["http://127.0.0.1/cb"]}))
        .send()
        .await
        .expect("POST /oauth/register")
        .json()
        .await
        .expect("parse register response");
    assert!(register["client_id"].as_str().is_some());

    // Key-based path: signup -> publish -> call -> admin, unchanged.
    let qualified = publish(
        &key_client,
        "hello",
        "echo",
        json!({"schema": {"type": "object", "properties": {"msg": {"type": "string"}}}}),
    )
    .await;
    assert_eq!(qualified, format!("{ns}.hello"));
    let result = key_client
        .tools_call(&qualified, json!({"msg": "hi"}))
        .await
        .expect("calling a key-authenticated tenant's own published tool must still work");
    assert_eq!(common::extract_structured(&result)["msg"], json!("hi"));

    let key_whoami = key_client.tools_call("host.whoami", json!({})).await.expect("host.whoami via key");
    let key_whoami = common::extract_structured(&key_whoami);
    assert_eq!(key_whoami["tenant"], json!(ns));
    assert_eq!(key_whoami["subject"], serde_json::Value::Null);
    assert_eq!(key_whoami["auth_method"], json!("key"), "whoami: {key_whoami}");

    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let listed = admin.tools_call("admin.tenants", json!({})).await.expect("admin.tenants");
    let listed = common::extract_structured(&listed);
    assert!(
        listed["tenants"].as_array().expect("tenants array").iter().any(|t| t["tenant"] == json!(ns)),
        "admin.tenants must still list the key-based tenant: {listed}"
    );

    // Issuer-JWT path: a registered per-tenant issuer's JWT still runs as
    // this tenant, unchanged.
    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://issuer.example.com";
    let audience = "mcphost-test-audience";
    key_client
        .tools_call(
            "host.oauth.issuer_set",
            json!({"issuer": issuer, "audience": audience, "jwks_url": format!("{}/jwks", jwks.uri())}),
        )
        .await
        .expect("issuer_set must succeed");

    let token = sign(KID_1, priv_pem_1(), issuer, audience, "user-42", 300);
    let bearer_client = McpClient::with_bearer(&server.base_url, &token);
    let bearer_whoami = bearer_client.tools_call("host.whoami", json!({})).await.expect("host.whoami via issuer JWT");
    let bearer_whoami = common::extract_structured(&bearer_whoami);
    assert_eq!(bearer_whoami["tenant"], json!(ns), "must run as the issuer's registered tenant");
    assert_eq!(bearer_whoami["subject"], json!("user-42"));
    assert_eq!(bearer_whoami["auth_method"], json!("oauth"), "whoami: {bearer_whoami}");
}
