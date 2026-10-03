//! PRD-mcphost-public-tool-url
//! AC1 (P0) — Given a tenant with a published echo tool, When
//! `host.tool_share(name = "echo", visibility = "url")` is called, Then the
//! result carries `url` matching `^https://[^/]+/x/[a-z2-7]{26}/echo$` and a
//! second identical call returns the same URL.
//!
//! The literal AC text pins an `https://` scheme; this suite's `TestServer`
//! binds plain `http://127.0.0.1:<port>` (no TLS in a local test process),
//! so this test checks the URL's shape against the server's own
//! `base_url` instead of a hardcoded scheme -- same convention
//! `tests/mcphost_webhook_inbox_ac02_signed_post_stores_row_and_fires_run.rs`
//! already uses for its own generated `url` field.

use crate::common;
use common::{TestServer, extract_structured, signup};
use serde_json::json;

fn is_base32_token(s: &str) -> bool {
    s.len() == 26 && s.chars().all(|c| matches!(c, 'a'..='z' | '2'..='7'))
}

#[tokio::test]
async fn tool_share_url_mints_a_stable_token_url() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC1 Tenant").await;
    let client = common::McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echo", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish ok");

    let first = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "echo", "visibility": "url"}))
            .await
            .expect("tool_share ok"),
    );
    let url = first["url"].as_str().expect("url field present").to_string();

    let prefix = format!("{}/x/", server.base_url);
    assert!(url.starts_with(&prefix), "url {url} must start with {prefix}");
    assert!(url.ends_with("/echo"), "url {url} must end with /echo");
    let token = url
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix("/echo"))
        .expect("token segment between prefix and /echo");
    assert!(
        is_base32_token(token),
        "token '{token}' must be exactly 26 lowercase base32 ([a-z2-7]) characters"
    );

    // A second identical call returns the exact same URL.
    let second = extract_structured(
        &client
            .tools_call("host.tool_share", json!({"name": "echo", "visibility": "url"}))
            .await
            .expect("tool_share ok (second call)"),
    );
    assert_eq!(second["url"], first["url"], "re-sharing must return the same URL");
}
