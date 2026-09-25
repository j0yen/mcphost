//! PRD-mcphost-abuse-guard-ban-list
//! AC3 (P0) — Given a ban with `public: false`, When the banned subject is
//! refused, Then the error carries no `reason`; with `public: true` it
//! carries the reason string.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer};
use serde_json::json;

#[tokio::test]
async fn public_flag_controls_whether_the_refusal_carries_a_reason() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);
    let anon = McpClient::new(&server.base_url);

    admin
        .tools_call(
            "admin.ban.add",
            json!({
                "subject_kind": "addr",
                "subject": "203.0.113.20",
                "ttl": "24h",
                "reason": "private flood",
                "public": false,
            }),
        )
        .await
        .expect("admin.ban.add private");
    admin
        .tools_call(
            "admin.ban.add",
            json!({
                "subject_kind": "addr",
                "subject": "203.0.113.21",
                "ttl": "24h",
                "reason": "public flood",
                "public": true,
            }),
        )
        .await
        .expect("admin.ban.add public");

    let private_err = anon
        .tools_call_with_header(
            "signup",
            json!({"name": "Privately Banned"}),
            ("x-forwarded-for", "203.0.113.20"),
        )
        .await
        .expect_err("banned address must be refused");
    assert_eq!(private_err.error_code.as_deref(), Some("banned"));
    assert!(
        !private_err.data.as_object().is_some_and(|o| o.contains_key("reason")),
        "a public: false ban must carry no reason key at all: {:?}",
        private_err.data
    );

    let public_err = anon
        .tools_call_with_header(
            "signup",
            json!({"name": "Publicly Banned"}),
            ("x-forwarded-for", "203.0.113.21"),
        )
        .await
        .expect_err("banned address must be refused");
    assert_eq!(public_err.error_code.as_deref(), Some("banned"));
    assert_eq!(
        public_err.data.get("reason").and_then(|v| v.as_str()),
        Some("public flood"),
        "a public: true ban must carry its reason verbatim: {:?}",
        public_err.data
    );
}
