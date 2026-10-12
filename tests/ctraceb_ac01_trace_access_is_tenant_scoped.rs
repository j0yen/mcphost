//! PRD-mcphost-call-trace-replay-b (ctraceb)
//! AC1 -- Given tenant A's `call_id`, When tenant B calls `host.call_trace`,
//! Then `forbidden`; Given an unknown or expired id, Then `trace_not_found`;
//! Given the admin bearer, Then A's trace.

use crate::ctrace;
use crate::common;

use common::{ADMIN_KEY, McpClient, publish, signup};
use serde_json::json;

#[tokio::test]
async fn ctraceb_another_tenant_is_forbidden_unknown_and_expired_are_not_found_and_admin_reads_any() {
    let _switch = ctrace::trace_switch().await;
    let fx = ctrace::start().await;
    publish(&fx.client, "ping", "echo", json!({"schema": {"type": "object"}})).await;
    let result = fx.tool_call("ping", json!({"q": "hello"})).await.expect("tenant A's call");
    let call_id = result["call_id"].as_str().expect("call_id").to_string();

    // The owner reads its own trace.
    let own = fx.trace(&fx.client, &call_id).await.expect("A reads its own trace");
    assert_eq!(own["call_id"], json!(call_id), "{own}");
    assert_eq!(own["tool"], json!("ping"), "{own}");
    assert_eq!(own["request"]["args"], json!({"q": "hello"}), "{own}");

    // Tenant B holds a valid key, and A's id.
    let (_, key_b) = signup(&fx.server.base_url, "tenant b").await;
    let client_b = McpClient::with_bearer(&fx.server.base_url, &key_b);
    let err = fx.trace(&client_b, &call_id).await.expect_err("B must not read A's trace");
    assert_eq!(err.error_code.as_deref(), Some("forbidden"), "{err:?}");

    // An id nobody ever had.
    let err = fx.trace(&fx.client, "987654321").await.expect_err("unknown id");
    assert_eq!(err.error_code.as_deref(), Some("trace_not_found"), "{err:?}");

    // The admin bearer reads A's trace.
    let admin = McpClient::with_bearer(&fx.server.base_url, ADMIN_KEY);
    let seen = fx.trace(&admin, &call_id).await.expect("admin reads A's trace");
    assert_eq!(seen["call_id"], json!(call_id), "{seen}");
    assert_eq!(seen["request"]["args"], json!({"q": "hello"}), "{seen}");

    // An expired trace (older than 24 h, swept by the housekeeping tick) is
    // gone for everyone, its `calls` row notwithstanding.
    let id: i64 = call_id.parse().expect("numeric id");
    let db = &fx.server.state.db;
    db.age_call_trace_for_test(id, mcphost::state::now_unix() - 2 * 86_400).await.expect("age the trace");
    mcphost::channels::tick_once(&fx.server.state).await.expect("housekeeping tick");
    let err = fx.trace(&fx.client, &call_id).await.expect_err("expired id");
    assert_eq!(err.error_code.as_deref(), Some("trace_not_found"), "{err:?}");
    let err = fx.trace(&admin, &call_id).await.expect_err("expired id, admin");
    assert_eq!(err.error_code.as_deref(), Some("trace_not_found"), "{err:?}");
    assert!(db.call_tool_name_for_test(id).await.expect("calls lookup").is_some(), "the calls row survives");
}
