//! PRD-mcphost-abuse-guard-ban-list
//! AC9 (P0) — Given a permanent ban request without the literal
//! `permanent: true`, When `admin.ban.add` is called with neither `ttl`
//! nor `permanent`, Then it is rejected with a validation error.

use crate::common;
use common::{ADMIN_KEY, McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn neither_ttl_nor_permanent_is_rejected() {
    let server = TestServer::start().await;
    let admin = McpClient::with_bearer(&server.base_url, ADMIN_KEY);

    let err = admin
        .tools_call(
            "admin.ban.add",
            json!({"subject_kind": "addr", "subject": "203.0.113.60", "reason": "no ttl or permanent"}),
        )
        .await
        .expect_err("neither ttl nor permanent: true must be rejected");
    assert_eq!(err.error_code.as_deref(), Some("invalid_params"));

    let list = admin
        .tools_call("admin.ban.list", json!({}))
        .await
        .expect("admin.ban.list");
    let rows = extract_structured(&list)["bans"].as_array().cloned().unwrap_or_default();
    assert!(rows.is_empty(), "a rejected admin.ban.add must not create a row: {rows:?}");

    // The literal `permanent: true` is honored: no expires_at, never
    // rejected.
    let ok = admin
        .tools_call(
            "admin.ban.add",
            json!({"subject_kind": "addr", "subject": "203.0.113.61", "reason": "forever", "permanent": true}),
        )
        .await
        .expect("permanent: true must be accepted");
    assert_eq!(extract_structured(&ok)["expires_at"], json!(null));

    // A truthy-looking but non-boolean permanent value doesn't count.
    let err2 = admin
        .tools_call(
            "admin.ban.add",
            json!({"subject_kind": "addr", "subject": "203.0.113.62", "reason": "not really", "permanent": "true"}),
        )
        .await
        .expect_err("a string \"true\" is not the literal boolean permanent: true");
    assert_eq!(err2.error_code.as_deref(), Some("invalid_params"));
}
