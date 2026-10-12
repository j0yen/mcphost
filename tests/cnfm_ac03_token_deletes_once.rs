//! PRD-mcphost-destructive-verb-confirmation-token
//! AC3 -- Given the dry run returned `confirm: c_…`, When the real call is
//! made with the same args and `confirm` within 10 min, Then the rows are
//! deleted and `confirm_tokens.used_unix` is set; When the same token is sent
//! again, Then `confirm_invalid {reason: "used"}` and nothing changes.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup};
use serde_json::{Value, json};

async fn seed_jobs(client: &McpClient, total: usize) {
    client
        .tools_call(
            "host.state.table_create",
            json!({"name": "jobs", "schema": {"n": "integer", "status": "text"}}),
        )
        .await
        .expect("table_create");
    let mut n = 0;
    while n < total {
        let end = (n + 200).min(total);
        let rows: Vec<Value> = (n..end).map(|i| json!({"n": i, "status": "done"})).collect();
        client
            .tools_call("host.state.insert", json!({"table": "jobs", "rows": rows}))
            .await
            .expect("insert batch");
        n = end;
    }
}

async fn count_jobs(client: &McpClient) -> usize {
    let result = client
        .tools_call("host.state.query", json!({"table": "jobs", "limit": 10_000}))
        .await
        .expect("state query");
    extract_structured(&result)["rows"].as_array().expect("rows").len()
}

#[tokio::test]
async fn a_dry_run_token_deletes_once_and_is_then_used() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Cnfm AC3 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    seed_jobs(&client, 300).await;

    let args = json!({"table": "jobs", "where": {"status": "done"}});
    let mut dry_args = args.clone();
    dry_args["dry_run"] = json!(true);
    let dry = extract_structured(
        &client
            .tools_call("host.state.delete_rows", dry_args)
            .await
            .expect("dry run"),
    );
    let token = dry["confirm"].as_str().expect("the dry run mints a token").to_string();
    assert!(token.starts_with("c_"), "{dry}");
    assert_eq!(count_jobs(&client).await, 300, "a dry run deletes nothing");

    let mut real_args = args.clone();
    real_args["confirm"] = json!(token);
    let done = extract_structured(
        &client
            .tools_call("host.state.delete_rows", real_args.clone())
            .await
            .expect("the confirmed call deletes"),
    );
    assert_eq!(done["deleted"], json!(300), "{done}");
    assert_eq!(count_jobs(&client).await, 0);

    // The token is recorded as used, by hash only.
    let conn = rusqlite::Connection::open(server.state.db.data_dir().join("mcphost.db")).expect("open db");
    let (used, stored): (Option<i64>, String) = conn
        .query_row("SELECT used_unix, token_hash FROM confirm_tokens", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("one token row");
    assert!(used.is_some(), "used_unix is set once the write ran");
    assert_ne!(stored, token, "the token itself is never stored");

    // The same token again changes nothing and says why.
    seed_jobs_again(&client).await;
    let again = client
        .tools_call("host.state.delete_rows", real_args)
        .await
        .expect_err("a used token is refused");
    assert_eq!(again.error_code.as_deref(), Some("confirm_invalid"), "{again:?}");
    assert_eq!(again.data["reason"], json!("used"), "{again:?}");
    assert_eq!(count_jobs(&client).await, 300, "a refused call deletes nothing");
}

/// Puts 300 rows back so the second use would have something to delete.
async fn seed_jobs_again(client: &McpClient) {
    let mut n = 0;
    while n < 300 {
        let end = (n + 200).min(300);
        let rows: Vec<Value> = (n..end).map(|i| json!({"n": i, "status": "done"})).collect();
        client
            .tools_call("host.state.insert", json!({"table": "jobs", "rows": rows}))
            .await
            .expect("re-insert batch");
        n = end;
    }
}
