//! PRD-mcphost-second-session-nudge
//! AC6 (P0) — Given a tenant whose last call was 3 days ago with two inbox
//! messages waiting, When it makes its next call, Then the `welcome_back`
//! envelope contains `since_you_left { calls: 0, runs: 0, inbox: 2, days:
//! 3 }`.

use crate::common;
use common::{McpClient, TestServer, extract_structured};
use serde_json::json;

#[tokio::test]
async fn welcome_back_reports_since_you_left_after_a_three_day_silence() {
    let server = TestServer::start().await;

    // The tenant whose next call will see `welcome_back`: signs up, makes
    // an authenticated call (so `last_seen_unix` is set), mints a personal
    // URL (same earlier-session setup `fcgift_ac03` uses).
    let session1 = McpClient::new(&server.base_url).with_session_continuity();
    let signup = extract_structured(
        &session1
            .tools_call("signup", json!({"name": "AC6 Tenant"}))
            .await
            .expect("signup"),
    );
    let namespace = signup["tenant"].as_str().expect("tenant").to_string();
    session1.tools_call("host.whoami", json!({})).await.expect("whoami on the earlier session");
    let rotated = extract_structured(
        &session1
            .tools_call("host.key_rotate", json!({}))
            .await
            .expect("host.key_rotate mints a personal URL"),
    );
    let url = rotated["url"].as_str().expect("url").to_string();
    let path = url.trim_start_matches(&server.base_url).to_string();

    let tenant = server
        .state
        .db
        .find_tenant_by_namespace(namespace)
        .await
        .expect("find tenant")
        .expect("tenant exists");

    // Two inbox messages waiting, sent by a second tenant while the first
    // is away (same `host.msg.send` flow `wake_ac1` uses).
    let (sender_ns, sender_key) = common::signup(&server.base_url, "AC6 Sender").await;
    let sender_client = McpClient::with_bearer(&server.base_url, &sender_key);
    for body in ["while you were away 1", "while you were away 2"] {
        sender_client
            .tools_call("host.msg.send", json!({"to": [tenant.namespace.clone()], "body": body}))
            .await
            .expect("msg.send");
    }
    let _ = sender_ns;

    // "last call 3 days ago": backdate `last_seen_unix` directly rather
    // than waiting out a real 3-day gap.
    let three_days_ago = mcphost::state::now_unix() - 3 * 86_400;
    server
        .state
        .db
        .set_tenant_stamp_for_test(tenant.id, "last_seen_unix", three_days_ago)
        .await
        .expect("backdate last_seen_unix");

    // A brand-new, URL-bound session's first call -- the one that must
    // carry `welcome_back.since_you_left`.
    let session2 = McpClient::new(&server.base_url).with_path(&path).with_session_continuity();
    let first_call = extract_structured(
        &session2
            .tools_call("host.whoami", json!({}))
            .await
            .expect("first call of the new URL-bound session"),
    );

    let since_you_left = &first_call["welcome_back"]["since_you_left"];
    assert_eq!(since_you_left["calls"], json!(0), "{first_call}");
    assert_eq!(since_you_left["runs"], json!(0), "{first_call}");
    assert_eq!(since_you_left["inbox"], json!(2), "{first_call}");
    assert_eq!(since_you_left["days"], json!(3), "{first_call}");
}
