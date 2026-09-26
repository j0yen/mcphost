//! PRD-mcphost-end-user-identity
//! AC1 (P0) — Given a bearer token with `sub=u1` validated by the OAuth PRD,
//! When a python-kind tool runs, Then its environment has
//! `MCPHOST_END_USER_ID=u1`, `MCPHOST_END_USER_METHOD=oauth`, and the
//! `calls` row has `end_user_subject=u1`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use crate::oauth;
use mcphost::sandbox;
use oauth::{KID_1, jwk_1, jwks_server, priv_pem_1, sign};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn oauth_end_user_reaches_python_env_and_calls_row() {
    // Requirement 8/9: this test runs a real python-kind tool via the
    // sandbox, which needs unprivileged user namespaces -- same
    // skip-clean-in-CI, fail-loud-elsewhere pattern every other
    // sandbox-dependent test in this suite uses.
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "AC1 Tenant").await;
    let key_client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "source": "import os\n\
            def main(args):\n\
            \treturn {\n\
            \t    \"id\": os.environ.get(\"MCPHOST_END_USER_ID\"),\n\
            \t    \"method\": os.environ.get(\"MCPHOST_END_USER_METHOD\"),\n\
            \t    \"has_id\": \"MCPHOST_END_USER_ID\" in os.environ,\n\
            \t}\n",
        "args_schema": {"type": "object"},
    });
    key_client
        .tools_call(
            "host.tool_publish",
            json!({"name": "whoami_tool", "kind": "python", "spec": spec}),
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

    let token = sign(KID_1, priv_pem_1(), issuer, audience, "u1", 300);
    let bearer_client = McpClient::with_bearer(&server.base_url, &token);

    let result = poll_until_ready(
        &bearer_client,
        &format!("{ns}.whoami_tool"),
        json!({}),
        Duration::from_secs(10),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let structured = extract_structured(&result);
    assert_eq!(structured["id"], json!("u1"), "MCPHOST_END_USER_ID must equal the JWT's sub: {structured}");
    assert_eq!(structured["method"], json!("oauth"), "{structured}");
    assert_eq!(structured["has_id"], json!(true), "{structured}");

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(ns)
        .await
        .unwrap()
        .expect("tenant");
    let (subject, issuer_col, method) = server
        .state
        .db
        .last_call_end_user_for_test(tenant.id, "whoami_tool".to_string())
        .await
        .unwrap()
        .expect("a calls row must exist for whoami_tool");
    assert_eq!(subject.as_deref(), Some("u1"), "calls.end_user_subject");
    assert_eq!(issuer_col.as_deref(), Some(issuer), "calls.end_user_issuer");
    assert_eq!(method.as_deref(), Some("oauth"), "calls.end_user_method");
}
