//! PRD-mcphost-first-hour-support-surface
//! AC1 (P0) — Given a call without a bearer key, When rejected, Then the
//! payload has `code=bearer_invalid`, a `request_id`, and `help_url` ending
//! `/help/bearer_invalid`, and GET of that URL on the in-process server
//! returns 200 with the code's paragraph.
//!
//! "A call without a bearer key" that reaches `bearer_invalid` specifically
//! (as opposed to `tenant_key_missing`) is one where a header WAS sent but
//! didn't resolve -- see `tests/autherr_ac4_bearer_invalid_unchanged.rs`'s
//! own second test for the fully-anonymous case, which is `tenant_key_missing`
//! instead (a different, already-covered AC); this file exercises the
//! `bearer_invalid` payload's own new fields.

use crate::common;
use common::{McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn bearer_invalid_carries_request_id_and_a_resolving_help_url() {
    let server = TestServer::start().await;
    let client = McpClient::with_bearer(&server.base_url, "not-a-real-key-at-all");

    let err = client
        .tools_call("admin.tenants", json!({}))
        .await
        .expect_err("an invalid bearer must be refused");
    assert_eq!(err.error_code.as_deref(), Some("bearer_invalid"));

    let request_id = err
        .data
        .get("request_id")
        .and_then(|v| v.as_str())
        .expect("payload must carry a request_id");
    assert!(!request_id.is_empty(), "request_id must not be blank");

    let help_url = err
        .data
        .get("help_url")
        .and_then(|v| v.as_str())
        .expect("bearer_invalid must carry a help_url");
    assert!(
        help_url.ends_with("/help/bearer_invalid"),
        "help_url must end /help/bearer_invalid: {help_url}"
    );
    assert!(
        help_url.starts_with(&server.base_url),
        "help_url must resolve on this same in-process server: {help_url}"
    );

    let page = reqwest::get(help_url).await.expect("GET the help page");
    assert_eq!(page.status(), reqwest::StatusCode::OK);
    let body = page.text().await.expect("help page body");
    assert!(!body.is_empty(), "help page must not be blank");
    assert!(
        body.contains("bearer_invalid"),
        "help page must name its own code: {body}"
    );
}
