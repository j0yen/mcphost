//! PRD-mcphost-end-user-identity
//! AC5 (P0) — Given two OAuth end users u1 and u2 of the same tenant, When
//! each writes `host.state.set{key:"k", end_user:"self"}` with its own
//! bearer, Then each reads back only its own value with
//! `end_user:"self"`, and a read with no `end_user` sees the tenant-wide
//! value (set separately), never either end user's value.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1, sign};
use serde_json::json;

#[tokio::test]
async fn each_end_user_reads_only_its_own_value_tenant_wide_read_stays_separate() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

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

    let token_u1 = sign(KID_1, priv_pem_1(), issuer, audience, "u1", 300);
    let token_u2 = sign(KID_1, priv_pem_1(), issuer, audience, "u2", 300);
    let client_u1 = McpClient::with_bearer(&server.base_url, &token_u1);
    let client_u2 = McpClient::with_bearer(&server.base_url, &token_u2);

    // A tenant-wide value, written with no end_user at all.
    key_client
        .tools_call("host.state.set", json!({"key": "k", "value": {"who": "tenant"}}))
        .await
        .expect("tenant-wide set");

    // u1 and u2 each write their own scoped value under the same key.
    client_u1
        .tools_call(
            "host.state.set",
            json!({"key": "k", "value": {"who": "u1"}, "end_user": "self"}),
        )
        .await
        .expect("u1 set");
    client_u2
        .tools_call(
            "host.state.set",
            json!({"key": "k", "value": {"who": "u2"}, "end_user": "self"}),
        )
        .await
        .expect("u2 set");

    let got_u1 = extract_structured(
        &client_u1
            .tools_call("host.state.get", json!({"key": "k", "end_user": "self"}))
            .await
            .expect("u1 get"),
    );
    assert_eq!(got_u1["value"], json!({"who": "u1"}), "{got_u1:?}");
    assert_eq!(got_u1["found"], json!(true));

    let got_u2 = extract_structured(
        &client_u2
            .tools_call("host.state.get", json!({"key": "k", "end_user": "self"}))
            .await
            .expect("u2 get"),
    );
    assert_eq!(got_u2["value"], json!({"who": "u2"}), "{got_u2:?}");
    assert_eq!(got_u2["found"], json!(true));

    // A read with no end_user must see only the tenant-wide value, never
    // either end user's scoped write.
    let got_tenant = extract_structured(
        &key_client
            .tools_call("host.state.get", json!({"key": "k"}))
            .await
            .expect("tenant-wide get"),
    );
    assert_eq!(got_tenant["value"], json!({"who": "tenant"}), "{got_tenant:?}");
    assert_eq!(got_tenant["found"], json!(true));
}

#[tokio::test]
async fn tenant_wide_read_of_an_unset_key_is_null_not_either_end_users_value() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC5 Tenant B").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

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

    let token_u1 = sign(KID_1, priv_pem_1(), issuer, audience, "u1", 300);
    let client_u1 = McpClient::with_bearer(&server.base_url, &token_u1);
    client_u1
        .tools_call(
            "host.state.set",
            json!({"key": "k", "value": {"who": "u1"}, "end_user": "self"}),
        )
        .await
        .expect("u1 set");

    // No tenant-wide write ever happened for this key -- a read with no
    // end_user must report not-found, not u1's scoped value.
    let got_tenant = extract_structured(
        &key_client
            .tools_call("host.state.get", json!({"key": "k"}))
            .await
            .expect("tenant-wide get"),
    );
    assert_eq!(got_tenant["found"], json!(false), "{got_tenant:?}");
    assert_eq!(got_tenant["value"], serde_json::Value::Null, "{got_tenant:?}");
}
