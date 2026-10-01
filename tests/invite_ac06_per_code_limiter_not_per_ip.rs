//! PRD-mcphost-invite-links
//! AC6 (P0) — Given six fresh sessions from one IP joining via one
//! invite within an hour, When each makes its first call, Then all six
//! succeed (per-code limiter, not per-IP) and a 21st within the hour is
//! refused with `invite_rate_limited`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn per_code_limiter_admits_twenty_then_refuses_the_rest() {
    let server = TestServer::start().await;
    let (ns_a, key_a) = signup(&server.base_url, "AC6 Inviter").await;
    let client_a = McpClient::with_bearer(&server.base_url, &key_a);

    client_a
        .tools_call(
            "host.tool_publish",
            json!({"name": "tt", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish t");
    // max_uses 50: the binding constraint under test is the per-code
    // 20/hour creation-event limiter, not `max_uses`.
    let created = client_a
        .tools_call("host.invite.create", json!({"share": ["tt"], "max_uses": 50}))
        .await
        .expect("host.invite.create");
    let url = extract_structured(&created)["url"].as_str().expect("url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();

    // First six -- and in fact the first twenty -- all succeed, all from
    // the same (loopback) test-server IP.
    for i in 0..6 {
        let session = McpClient::new(&server.base_url)
            .with_path(&path)
            .with_session_continuity();
        session
            .tools_call("host.tool_call", json!({"name": format!("{ns_a}.tt"), "args": {}}))
            .await
            .unwrap_or_else(|e| panic!("join #{i} must succeed: {} {}", e.code, e.message));
    }
    for i in 6..20 {
        let session = McpClient::new(&server.base_url)
            .with_path(&path)
            .with_session_continuity();
        session
            .tools_call("host.tool_call", json!({"name": format!("{ns_a}.tt"), "args": {}}))
            .await
            .unwrap_or_else(|e| panic!("join #{i} must succeed: {} {}", e.code, e.message));
    }

    // The 21st within the hour is refused.
    let session21 = McpClient::new(&server.base_url)
        .with_path(&path)
        .with_session_continuity();
    let err = session21
        .tools_call("host.tool_call", json!({"name": format!("{ns_a}.tt"), "args": {}}))
        .await
        .expect_err("the 21st join within the hour must be refused");
    assert_eq!(err.error_code.as_deref(), Some("invite_rate_limited"), "{err:?}");
}
