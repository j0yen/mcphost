//! PRD-mcphost-url-bound-tenants
//! AC7 (P0) — Given a request on `/u/<secret>/mcp` carrying an
//! `Authorization: Bearer` for a different tenant, When dispatched, Then
//! the error class is `auth_conflict` and no call is executed.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn different_tenants_header_and_path_secret_conflict() {
    let server = TestServer::start().await;

    // Tenant A gets a URL.
    let session_a = McpClient::new(&server.base_url).with_session_continuity();
    session_a
        .tools_call("signup", json!({"name": "AC7 Tenant A"}))
        .await
        .expect("signup A");
    let rotated_a = extract_structured(
        &session_a
            .tools_call("host.key_rotate", json!({}))
            .await
            .expect("host.key_rotate mints A's URL"),
    );
    let url_a = rotated_a["url"].as_str().expect("url").to_string();
    let path_a = url_a.trim_start_matches(&server.base_url).to_string();

    // Tenant B has its own, different key.
    let (ns_b, key_b) = signup(&server.base_url, "AC7 Tenant B").await;

    // A's path secret, but B's key in the Authorization header.
    let mut client = McpClient::new(&server.base_url).with_path(&path_a);
    client.bearer = Some(key_b);

    let err = client
        .tools_call("host.whoami", json!({}))
        .await
        .expect_err("a different tenant's bearer alongside this path's own secret must be refused");
    assert_eq!(err.error_code.as_deref(), Some("auth_conflict"), "{err:?}");
    assert!(
        !err.message.contains(&ns_b),
        "the refusal must not echo the conflicting tenant's namespace: {}",
        err.message
    );
}

#[tokio::test]
async fn same_tenants_header_and_path_secret_do_not_conflict() {
    let server = TestServer::start().await;
    let session = McpClient::new(&server.base_url).with_session_continuity();
    let signed_up = session
        .tools_call("signup", json!({"name": "AC7 Same Tenant"}))
        .await
        .expect("signup");
    let namespace = extract_structured(&signed_up)["tenant"].as_str().unwrap().to_string();
    let rotated = extract_structured(
        &session
            .tools_call("host.key_rotate", json!({}))
            .await
            .expect("host.key_rotate mints a URL"),
    );
    let url = rotated["url"].as_str().expect("url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();
    let key = rotated["key"].as_str().expect("key").to_string();

    // The SAME tenant's own key alongside its own path secret: no conflict.
    let mut client = McpClient::new(&server.base_url).with_path(&path);
    client.bearer = Some(key);
    let whoami = extract_structured(
        &client
            .tools_call("host.whoami", json!({}))
            .await
            .expect("same tenant's own key alongside its own URL must not conflict"),
    );
    assert_eq!(whoami["tenant"], json!(namespace), "{whoami}");
}
