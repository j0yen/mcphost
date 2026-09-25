//! PRD-mcphost-abuse-guard-ban-list
//! AC1 (P0) — Given `admin.ban.add {subject_kind: "addr", subject:
//! "203.0.113.9", ttl: "24h", reason: "flood"}`, When `signup` is called
//! from that address, Then the response is error `banned` with
//! `expires_at` set and no `signup_events` row is written; from another
//! address signup succeeds.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn banned_address_refuses_signup_other_addresses_unaffected() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let anon = McpClient::new(&server.base_url);

    admin
        .tools_call(
            "admin.ban.add",
            json!({
                "subject_kind": "addr",
                "subject": "203.0.113.9",
                "ttl": "24h",
                "reason": "flood",
            }),
        )
        .await
        .expect("admin.ban.add");

    let err = anon
        .tools_call_with_header(
            "signup",
            json!({"name": "Banned Agent"}),
            ("x-forwarded-for", "203.0.113.9"),
        )
        .await
        .expect_err("a signup from the banned address must be refused");
    assert_eq!(err.error_code.as_deref(), Some("banned"));
    assert!(
        err.data.get("expires_at").and_then(|v| v.as_i64()).is_some(),
        "a ttl-bound ban's refusal must carry a non-null expires_at: {:?}",
        err.data
    );

    let since = mcphost::state::now_unix() - 60;
    let banned_count = server
        .state
        .db
        .signup_count_since("203.0.113.9".to_string(), since)
        .await
        .expect("query signup_events");
    assert_eq!(
        banned_count, 0,
        "a banned signup attempt must not write a signup_events row"
    );

    anon.tools_call_with_header(
        "signup",
        json!({"name": "Clean Agent"}),
        ("x-forwarded-for", "203.0.113.10"),
    )
    .await
    .unwrap_or_else(|e| panic!("signup from an unbanned address should succeed: {e:?}"));
    let clean_count = server
        .state
        .db
        .signup_count_since("203.0.113.10".to_string(), since)
        .await
        .expect("query signup_events");
    assert_eq!(clean_count, 1, "the unbanned address's signup must be recorded");
}
