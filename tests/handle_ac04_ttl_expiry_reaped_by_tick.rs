//! PRD-mcphost-result-handles
//! AC4 -- Given `ttl_s: 1`, When time passes and the tick has run, Then
//! the handle is gone from `host.table.handles`, its table is dropped
//! from the file, and `bytes_used` fell accordingly.
//!
//! Drives `handles::tick_once` directly rather than waiting on the real
//! 30s background cadence -- same deterministic-tick convention
//! `tables_model::tick_once`'s own AC5 test already uses; only the TTL
//! itself (`ttl_s: 1`, with a short real sleep past it) needs real wall
//! time, not the tick's own period.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn expired_handle_is_reaped_by_tick_and_bytes_used_falls() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "ResultHandles AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "events", "columns": {"n": "integer"}}))
        .await
        .expect("create events");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "events", "rows": (0..50).map(|n| json!({"n": n})).collect::<Vec<_>>()}),
        )
        .await
        .expect("append");

    let materialize = extract_structured(
        &client
            .tools_call("host.table.query", json!({"sql": "SELECT * FROM events", "handle": true, "ttl_s": 1}))
            .await
            .expect("materialize"),
    );
    let handle = materialize["handle"].as_str().expect("handle").to_string();

    let before = extract_structured(&client.tools_call("host.table.handles", json!({})).await.expect("handles"));
    assert_eq!(before["handles"].as_array().unwrap().len(), 1, "before: {before}");
    let bytes_used_before = before["bytes_used"].as_i64().expect("bytes_used");
    assert!(bytes_used_before > 0, "before: {before}");

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    mcphost::handles::tick_once(&server.state).await.expect("tick_once");

    let after = extract_structured(&client.tools_call("host.table.handles", json!({})).await.expect("handles"));
    assert_eq!(after["handles"].as_array().unwrap().len(), 0, "after: {after}");
    assert_eq!(after["bytes_used"], 0, "bytes_used must fall to 0 once the only handle is reaped: {after}");

    // The table itself is gone from the file, not just the meta row --
    // referencing it now reads as an ordinary missing handle.
    let err = client
        .tools_call("host.table.query", json!({"sql": format!("SELECT * FROM {handle}")}))
        .await
        .expect_err("dropped handle must be gone");
    assert_eq!(err.error_code.as_deref(), Some("handle_not_found"), "err: {err:?}");
}
