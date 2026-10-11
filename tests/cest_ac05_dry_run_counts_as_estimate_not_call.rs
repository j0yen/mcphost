//! PRD-mcphost-call-cost-estimate
//! AC5 -- Given a dry run, When `host.usage(window: "1m")` is read, Then
//! `calls` is unchanged, `estimates` is 1, and `SELECT count(*) FROM calls`
//! is unchanged.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::json;

#[tokio::test]
async fn a_dry_run_moves_estimates_and_nothing_in_calls() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Cest Usage").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "echoer", "kind": "echo", "spec": {"schema": {"type": "object"}}}),
        )
        .await
        .expect("publish");
    // One real call so `calls` is not trivially zero.
    client.tools_call("host.tool_call", json!({"name": "echoer", "args": {}})).await.expect("real call");

    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open raw db");
    let rows = |conn: &rusqlite::Connection| -> i64 {
        conn.query_row("SELECT COUNT(*) FROM calls", [], |r| r.get(0)).expect("count calls")
    };
    let usage = || async {
        extract_structured(&client.tools_call("host.usage", json!({"window": "1m"})).await.expect("host.usage"))
    };

    let before = usage().await;
    let rows_before = rows(&conn);
    assert_eq!(before["calls"], json!(1), "{before}");
    assert_eq!(before["estimates"], json!(0), "{before}");

    let dry = extract_structured(
        &client
            .tools_call("host.tool_call", json!({"name": "echoer", "args": {}, "dry_run": true}))
            .await
            .expect("dry run"),
    );
    assert_eq!(dry["fits"], json!(true), "{dry}");

    let after = usage().await;
    assert_eq!(after["calls"], before["calls"], "a dry run is not a call: {after}");
    assert_eq!(after["estimates"], json!(1), "{after}");
    assert_eq!(rows(&conn), rows_before, "a dry run writes no calls row");
}
