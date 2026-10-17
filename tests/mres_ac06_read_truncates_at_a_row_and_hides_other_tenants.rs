//! AC6 (P0) -- PRD-mcphost-resources-for-tenant-data. Given a table whose JSON
//! exceeds `resource_read_bytes_max`, When read, Then the body is cut at the
//! last complete row under the ceiling and `_meta.truncated` is `true`;
//! Given another tenant's table name, Then `-32002`, never `forbidden`.

use crate::common::{McpClient, TestServer, signup};
use serde_json::{Value, json};

const CEILING: usize = 2000;

/// `resources/read` with no declared protocol version (so rmcp leaves
/// `-32002` as is; see mres_ac01).
async fn raw_read(base_url: &str, key: &str, uri: &str) -> Value {
    reqwest::Client::new()
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Mcp-Method", "resources/read")
        .header("Mcp-Name", uri)
        .header("Authorization", format!("Bearer {key}"))
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "resources/read", "params": {"uri": uri}}))
        .send()
        .await
        .expect("send")
        .json()
        .await
        .expect("json")
}

#[tokio::test]
async fn mres_ac06_oversized_table_is_cut_at_a_row_and_flagged() {
    let server = TestServer::start_with_plans(|catalog| {
        for plan in &mut catalog.plans {
            plan.resource_read_bytes_max = CEILING as i64;
        }
    })
    .await;
    let (_ns, key) = signup(&server.base_url, "mres ac06 owner").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"n": "integer", "note": "text"}}))
        .await
        .expect("create");
    let rows: Vec<Value> = (0..100).map(|n| json!({"n": n, "note": "x".repeat(30)})).collect();
    client.tools_call("host.table.append", json!({"table": "orders", "rows": rows})).await.expect("append");

    let reply = raw_read(&server.base_url, &key, "mcphost://table/orders").await;
    let content = &reply["result"]["contents"][0];
    assert_eq!(content["_meta"]["truncated"], true, "{reply}");
    assert_eq!(reply["result"]["_meta"]["truncated"], true, "{reply}");
    let text = content["text"].as_str().expect("text");
    assert!(text.len() <= CEILING, "body {} bytes exceeds the {CEILING} ceiling", text.len());
    let body: Value = serde_json::from_str(text).expect("a cut body is still valid JSON");
    let kept = body["rows"].as_array().expect("rows");
    assert!(!kept.is_empty() && kept.len() < 100, "cut, not emptied: {} rows", kept.len());
    for (i, row) in kept.iter().enumerate() {
        assert_eq!(row["n"], i as i64, "the kept rows are a whole-row prefix in order");
    }
    // One more row would not have fit: the cut is at the LAST complete row.
    let one_more = serde_json::to_string(&json!({"n": kept.len(), "note": "x".repeat(30)})).unwrap().len();
    assert!(text.len() + one_more + 1 > CEILING, "{} + {one_more} would still fit the ceiling", text.len());

    // Under the ceiling nothing is flagged.
    client.tools_call("host.table.create", json!({"name": "tiny", "columns": {"n": "integer"}})).await.expect("create tiny");
    client.tools_call("host.table.append", json!({"table": "tiny", "rows": [{"n": 1}]})).await.expect("append tiny");
    let small = raw_read(&server.base_url, &key, "mcphost://table/tiny").await;
    assert!(small["result"]["contents"][0].get("_meta").is_none(), "{small}");
}

#[tokio::test]
async fn mres_ac06_another_tenants_table_is_32002_never_forbidden() {
    let server = TestServer::start().await;
    let (_a, key_a) = signup(&server.base_url, "mres ac06 a").await;
    let (_b, key_b) = signup(&server.base_url, "mres ac06 b").await;
    McpClient::with_bearer(&server.base_url, &key_a)
        .tools_call("host.table.create", json!({"name": "orders", "columns": {"n": "integer"}}))
        .await
        .expect("a creates orders");

    let theirs = raw_read(&server.base_url, &key_b, "mcphost://table/orders").await;
    assert_eq!(theirs["error"]["code"], -32002, "{theirs}");
    assert_ne!(theirs["error"]["data"]["error_code"], "forbidden", "{theirs}");
    let nonexistent = raw_read(&server.base_url, &key_b, "mcphost://table/never_made").await;
    assert_eq!(theirs["error"]["code"], nonexistent["error"]["code"], "existence must not leak");
    assert_eq!(theirs["error"]["message"].as_str().map(|m| m.replace("orders", "X")), nonexistent["error"]["message"].as_str().map(|m| m.replace("never_made", "X")));
}
