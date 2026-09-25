//! PRD-mcphost-oauth-resource-server
//! AC2 (P0) — Given a tenant registered issuer I with audience A and a
//! JWKS URL served by a test server, When a JWT signed by that JWKS with
//! `iss=I, aud=A, exp` in the future calls `host.state.get`, Then the call
//! runs as that tenant and `subject` equals the JWT `sub`.

use crate::common;
use common::{McpClient, TestServer, signup};

use crate::oauth;
use oauth::{KID_1, priv_pem_1, jwk_1, jwks_server, sign};

#[tokio::test]
async fn valid_bearer_jwt_runs_as_the_registered_tenant() {
    let server = TestServer::start().await;
    let (ns, key) = signup(&server.base_url, "OAuth Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://issuer.example.com";
    let audience = "mcphost-test-audience";
    key_client
        .tools_call(
            "host.oauth.issuer_set",
            serde_json::json!({
                "issuer": issuer,
                "audience": audience,
                "jwks_url": format!("{}/jwks", jwks.uri()),
            }),
        )
        .await
        .expect("issuer_set must succeed");

    // Seed a value through the key path so the bearer path's read proves
    // it landed in the SAME tenant's state namespace, not a fresh one.
    key_client
        .tools_call("host.state.set", serde_json::json!({"key": "greeting", "value": "hi"}))
        .await
        .expect("state.set via key");

    let token = sign(KID_1, priv_pem_1(), issuer, audience, "user-42", 300);
    let bearer_client = McpClient::with_bearer(&server.base_url, &token);

    let result = bearer_client
        .tools_call("host.state.get", serde_json::json!({"key": "greeting"}))
        .await
        .expect("host.state.get via OAuth bearer must succeed");
    let structured = common::extract_structured(&result);
    assert_eq!(structured["found"], serde_json::json!(true));
    assert_eq!(structured["value"], serde_json::json!("hi"));

    let whoami = bearer_client
        .tools_call("host.whoami", serde_json::json!({}))
        .await
        .expect("host.whoami via OAuth bearer must succeed");
    let whoami = common::extract_structured(&whoami);
    assert_eq!(whoami["tenant"], serde_json::json!(ns), "must run as the issuer's registered tenant");
    assert_eq!(
        whoami["subject"],
        serde_json::json!("user-42"),
        "subject must equal the JWT's sub claim: {whoami}"
    );
}
