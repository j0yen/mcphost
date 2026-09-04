//! AC2 — Given a spec whose `url` is `http://` or resolves to `10.0.0.5`,
//! `127.0.0.1`, `169.254.169.254` or `mcphost.internal`, When published,
//! Then the publish is rejected with `host_not_allowed` naming `url`.
//!
//! Uses the *strict* (production) `http` kind policy -- this is exactly the
//! policy AC2 is about, so it must not be relaxed the way the functional
//! test suite's `http_kind_registry()` is.

mod common;
use common::{FixedLookup, McpClient, TestServer, http_kind_registry_strict, signup};
use serde_json::json;

async fn publish_is_rejected(url: &str) -> Option<String> {
    let server = TestServer::start_with_kinds(http_kind_registry_strict(FixedLookup(
        std::collections::HashMap::new(),
    )))
    .await;
    let (_ns, key) = signup(&server.base_url, "SSRF Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": url,
        "args_schema": {"type": "object"},
    });
    let err = client
        .tools_call(
            "host.tool_publish",
            json!({"name": "bad_tool", "kind": "http", "spec": spec}),
        )
        .await
        .expect_err(&format!("publishing url={url} must be rejected"));
    err.error_code
}

#[tokio::test]
async fn plain_http_is_rejected() {
    let code = publish_is_rejected("http://example.com/").await;
    assert_eq!(code.as_deref(), Some("host_not_allowed"));
}

#[tokio::test]
async fn rfc1918_literal_is_rejected() {
    let code = publish_is_rejected("https://10.0.0.5/").await;
    assert_eq!(code.as_deref(), Some("host_not_allowed"));
}

#[tokio::test]
async fn loopback_literal_is_rejected() {
    let code = publish_is_rejected("https://127.0.0.1/").await;
    assert_eq!(code.as_deref(), Some("host_not_allowed"));
}

#[tokio::test]
async fn link_local_metadata_literal_is_rejected() {
    let code = publish_is_rejected("https://169.254.169.254/").await;
    assert_eq!(code.as_deref(), Some("host_not_allowed"));
}

#[tokio::test]
async fn dot_internal_suffix_is_rejected() {
    let code = publish_is_rejected("https://mcphost.internal/").await;
    assert_eq!(code.as_deref(), Some("host_not_allowed"));
}

#[tokio::test]
async fn a_normal_public_host_is_accepted() {
    // Negative control: the rejection above is about these specific hosts,
    // not about `host.tool_publish` failing generally.
    let server = TestServer::start_with_kinds(http_kind_registry_strict(FixedLookup(
        std::collections::HashMap::new(),
    )))
    .await;
    let (_ns, key) = signup(&server.base_url, "Fine Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    let spec = json!({
        "method": "GET",
        "url": "https://api.example.com/v1/ping",
        "args_schema": {"type": "object"},
    });
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "ping", "kind": "http", "spec": spec}),
        )
        .await
        .expect("a normal public https host must publish fine");
}
