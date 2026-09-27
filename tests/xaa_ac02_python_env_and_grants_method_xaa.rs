//! PRD-mcphost-enterprise-managed-auth
//! AC2 (P0) — Given that token, When it calls a python-kind tool on
//! `/t/acme/mcp`, Then `MCPHOST_END_USER_METHOD=enterprise_assertion` and
//! `MCPHOST_END_USER_EMAIL` are set and `host.oauth.grants` shows the grant
//! with `method: xaa`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};

use crate::oauth;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1};

use crate::assertion;
use assertion::{fresh_jti, now_unix, sign_assertion};

use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::Duration;

#[tokio::test]
async fn assertion_derived_token_reaches_python_env_and_grants_list() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Acme Corp").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import os\n\
            def main(args):\n\
            \treturn {\n\
            \t    \"method\": os.environ.get(\"MCPHOST_END_USER_METHOD\"),\n\
            \t    \"email\": os.environ.get(\"MCPHOST_END_USER_EMAIL\"),\n\
            \t    \"has_email\": \"MCPHOST_END_USER_EMAIL\" in os.environ,\n\
            \t}\n",
        "args_schema": {"type": "object"},
    });
    key_client
        .tools_call("host.tool_publish", json!({"name": "whoami_tool", "kind": "python", "spec": spec}))
        .await
        .expect("publish ok");

    let jwks = jwks_server(jwk_1()).await;
    let issuer = "https://idp.acme-test.example.com";
    key_client
        .tools_call(
            "host.oauth.trusted_issuer_set",
            json!({
                "issuer": issuer,
                "jwks_url": format!("{}/jwks", jwks.uri()),
                "client_id": "claude-enterprise",
            }),
        )
        .await
        .expect("trusted_issuer_set must succeed");

    let resource = format!("{}/t/{ns}/mcp", server.base_url);
    let now = now_unix();
    let claims = json!({
        "iss": issuer,
        "aud": server.base_url,
        "sub": "okta|e1",
        "email": "e1@acme.test",
        "iat": now,
        "exp": now + 300,
        "jti": fresh_jti(),
    });
    let assertion_jwt = sign_assertion(KID_1, priv_pem_1(), &claims);

    let http = reqwest::Client::new();
    let token_resp: Value = http
        .post(format!("{}/oauth/token", server.base_url))
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion_jwt.as_str()),
            ("client_id", "claude-enterprise"),
            ("resource", resource.as_str()),
        ])
        .send()
        .await
        .expect("POST /oauth/token")
        .json()
        .await
        .expect("parse token response");
    let access_token = token_resp["access_token"].as_str().expect("access_token present").to_string();

    // AC2: calls on the tenant's own per-tenant resource path, not `/mcp`.
    let tenant_base = format!("{}/t/{ns}", server.base_url);
    let bearer_client = McpClient::with_bearer(&tenant_base, &access_token);
    let result = poll_until_ready(&bearer_client, &format!("{ns}.whoami_tool"), json!({}), Duration::from_secs(10))
        .await
        .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    assert_eq!(structured["method"], json!("enterprise_assertion"), "{structured}");
    assert_eq!(structured["email"], json!("e1@acme.test"), "{structured}");
    assert_eq!(structured["has_email"], json!(true), "{structured}");

    let grants = key_client.tools_call("host.oauth.grants", json!({})).await.expect("host.oauth.grants");
    let grants_structured = extract_structured(&grants);
    let grants_arr = grants_structured["grants"].as_array().expect("grants array");
    assert!(
        grants_arr.iter().any(|g| g["method"] == json!("xaa")),
        "host.oauth.grants must show the assertion-derived grant with method xaa: {grants_arr:?}"
    );
}
