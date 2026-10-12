//! PRD-mcphost-destructive-verb-confirmation-token
//! AC4 -- Given a token minted for `{table: "jobs", where: {status: "done"}}`,
//! When the real call changes `where` to `{}`, Then `confirm_invalid
//! {reason: "args_changed"}`; When 100 new rows are appended between dry run
//! and real call, Then `confirm_invalid {reason: "state_changed"}` with a
//! fresh `would`; When 601 s pass, Then `reason: "expired"`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

async fn append_jobs(client: &McpClient, from: usize, to: usize) {
    let mut n = from;
    while n < to {
        let end = (n + 200).min(to);
        let rows: Vec<Value> = (n..end).map(|i| json!({"n": i, "status": "done"})).collect();
        client
            .tools_call("host.state.insert", json!({"table": "jobs", "rows": rows}))
            .await
            .expect("insert batch");
        n = end;
    }
}

async fn mint(client: &McpClient, filter: Value) -> String {
    let dry = extract_structured(
        &client
            .tools_call(
                "host.state.delete_rows",
                json!({"table": "jobs", "where": filter, "dry_run": true}),
            )
            .await
            .expect("dry run"),
    );
    dry["confirm"].as_str().expect("the dry run mints a token").to_string()
}

#[tokio::test]
async fn a_token_is_bound_to_its_args_the_state_it_saw_and_ten_minutes() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Cnfm AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.state.table_create",
            json!({"name": "jobs", "schema": {"n": "integer", "status": "text"}}),
        )
        .await
        .expect("table_create");
    append_jobs(&client, 0, 300).await;
    let done = json!({"status": "done"});

    // Different arguments: args_changed, nothing deleted.
    let token = mint(&client, done.clone()).await;
    let err = client
        .tools_call("host.state.delete_rows", json!({"table": "jobs", "where": {}, "confirm": token}))
        .await
        .expect_err("a token for another filter is refused");
    assert_eq!(err.error_code.as_deref(), Some("confirm_invalid"), "{err:?}");
    assert_eq!(err.data["reason"], json!("args_changed"), "{err:?}");

    // The state moved under the token: state_changed with a fresh `would`.
    // (The refused call above did not consume the token.)
    append_jobs(&client, 300, 400).await;
    let err = client
        .tools_call("host.state.delete_rows", json!({"table": "jobs", "where": done, "confirm": token}))
        .await
        .expect_err("rows changed since the dry run");
    assert_eq!(err.error_code.as_deref(), Some("confirm_invalid"), "{err:?}");
    assert_eq!(err.data["reason"], json!("state_changed"), "{err:?}");
    assert_eq!(err.data["would"]["rows"][0]["delete"], json!(400), "fresh would: {err:?}");

    // 601 s later the token has expired, whatever the state.
    let token = mint(&client, done.clone()).await;
    server.state.confirm_clock_offset_secs.store(601, Ordering::Relaxed);
    let err = client
        .tools_call("host.state.delete_rows", json!({"table": "jobs", "where": done, "confirm": token}))
        .await
        .expect_err("an old token is refused");
    assert_eq!(err.error_code.as_deref(), Some("confirm_invalid"), "{err:?}");
    assert_eq!(err.data["reason"], json!("expired"), "{err:?}");

    // Through all of it nothing was deleted.
    let rows = extract_structured(
        &client
            .tools_call("host.state.query", json!({"table": "jobs", "limit": 10_000}))
            .await
            .expect("query"),
    );
    assert_eq!(rows["rows"].as_array().expect("rows").len(), 400);
}
