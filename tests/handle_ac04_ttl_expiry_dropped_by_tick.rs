//! PRD-mcphost-result-handles
//! AC4 -- Given `ttl_s: 1`, When 61 s pass and the tick has run, Then the
//! handle is gone from `host.table.handles`, its table is dropped from
//! the file, and `bytes_used` fell accordingly.
//!
//! Drives `tables::tick_once` directly rather than waiting on the real
//! 30s background cadence -- same deterministic-tick convention
//! `tables_model::tick_once`/`bans::tick_once` already use in this suite.
//! `ttl_s: 1` plus a real ~1.2s sleep stands in for "61 s pass" -- what
//! AC4 actually exercises is "past its own expiry, when the tick runs",
//! not a literal 61-second wait.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn expired_handle_is_dropped_by_the_tick() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Handle AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.table.create", json!({"name": "readings", "columns": {"v": "real"}}))
        .await
        .expect("create readings");
    client
        .tools_call(
            "host.table.append",
            json!({"table": "readings", "rows": (0..50).map(|i| json!({"v": i as f64})).collect::<Vec<_>>()}),
        )
        .await
        .expect("append readings");

    let summary = extract_structured(
        &client
            .tools_call(
                "host.table.query",
                json!({"sql": "SELECT * FROM readings", "handle": true, "ttl_s": 1}),
            )
            .await
            .expect("materialize handle with ttl_s=1"),
    );
    let handle = summary["handle"].as_str().expect("handle name").to_string();

    let with_handle_bytes = extract_structured(
        &client.tools_call("host.table.list", json!({})).await.expect("host.table.list with handle"),
    )["bytes_used"]
        .as_i64()
        .expect("bytes_used with handle");

    // Not checked "still listed" immediately: ttl_s: 1 is too tight a
    // window for that to be a non-racy assertion against a real HTTP
    // round trip -- AC4's own claim is about the state *after* expiry
    // plus a tick, which the rest of this test proves.
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    mcphost::tables::tick_once(&server.state).await.expect("tick_once");

    let listed_after = extract_structured(
        &client.tools_call("host.table.handles", json!({})).await.expect("host.table.handles after expiry"),
    );
    assert!(
        !listed_after["handles"]
            .as_array()
            .expect("handles array")
            .iter()
            .any(|h| h["handle"] == json!(handle)),
        "an expired handle must be gone from host.table.handles after the tick: {listed_after}"
    );

    // Its table is really dropped from the file, not just hidden from the
    // listing -- querying it now reads exactly like it was never created
    // (AC3's same error).
    let err = client
        .tools_call("host.table.query", json!({"sql": format!("SELECT * FROM {handle}")}))
        .await
        .expect_err("querying a dropped handle must be refused");
    assert_eq!(err.error_code.as_deref(), Some("handle_not_found"), "error: {err:?}");

    let after_bytes = extract_structured(
        &client.tools_call("host.table.list", json!({})).await.expect("host.table.list after"),
    )["bytes_used"]
        .as_i64()
        .expect("bytes_used after");
    assert!(
        after_bytes < with_handle_bytes,
        "bytes_used must have fallen after the handle's table was dropped: \
         with_handle={with_handle_bytes} after={after_bytes}"
    );
}
