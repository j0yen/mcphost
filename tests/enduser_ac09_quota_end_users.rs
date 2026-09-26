//! PRD-mcphost-end-user-identity
//! AC9 (P0) — Given the free plan's `end_users_max=100`, When a 101st
//! distinct subject writes state within the trailing 30 days, Then it is
//! rejected `quota_end_users`, while reads (and writes from already-active
//! subjects) still work.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn the_101st_distinct_end_user_is_quota_rejected_reads_still_work() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "AC9 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    // 100 distinct subjects each write once -- an explicit end_user is
    // allowed here because this key-based call carries no identity of its
    // own (same AC7 branch).
    for i in 1..=100 {
        client
            .tools_call(
                "host.state.set",
                json!({"key": "k", "value": {"n": i}, "end_user": format!("u{i}")}),
            )
            .await
            .unwrap_or_else(|e| panic!("subject u{i} must be allowed: {} {}", e.code, e.message));
    }

    // The 101st distinct subject is rejected.
    let err = client
        .tools_call(
            "host.state.set",
            json!({"key": "k", "value": {"n": 101}, "end_user": "u101"}),
        )
        .await
        .expect_err("the 101st distinct end user must be quota-rejected");
    assert_eq!(err.error_code.as_deref(), Some("quota_end_users"), "{err:?}");

    // An already-active subject (already counted, not new) may still
    // write past the quota.
    client
        .tools_call(
            "host.state.set",
            json!({"key": "k", "value": {"n": 999}, "end_user": "u1"}),
        )
        .await
        .expect("an already-active subject must still be able to write");

    // Reads keep working, for both an already-active subject and the
    // subject that was just quota-rejected on write.
    let got = extract_structured(
        &client
            .tools_call("host.state.get", json!({"key": "k", "end_user": "u1"}))
            .await
            .expect("read for an already-active subject must still work"),
    );
    assert_eq!(got["value"], json!({"n": 999}), "{got:?}");

    let got = extract_structured(
        &client
            .tools_call("host.state.get", json!({"key": "k", "end_user": "u101"}))
            .await
            .expect("read for the quota-rejected subject must still work"),
    );
    assert_eq!(got["found"], json!(false), "{got:?}");
}
