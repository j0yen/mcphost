//! PRD-mcphost-trusted-discovery-catalog-receipts AC3 (P0) -- a catalog entry
//! carries `calls_30d {n, ok}`: the cross-tenant calls of the last 30 days.
//! The plain view leaves out synthetic-marked callers, the synthetic view
//! counts every other tenant, the owner's own calls and calls older than 30
//! days count in neither, and no entry names a caller or a tenant id.

use crate::common;
use common::{McpClient, TestServer, extract_structured, signup_plain, signup_with_synthetic_header};
use rusqlite::Connection;
use serde_json::{Value, json};

async fn entry(client: &McpClient, full_name: &str) -> Value {
    let found = extract_structured(&client.tools_call("host.catalog.search", json!({"q": "tdisc ac3"})).await.expect("search"));
    let tools = found["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), 1, "{found}");
    assert_eq!(tools[0]["name"], json!(full_name), "{found}");
    tools[0].clone()
}

#[tokio::test]
async fn tdisc_ac03() {
    let server = TestServer::start().await;
    let (owner_ns, owner_key) = signup_plain(&server.base_url, "Tdisc AC3 Owner").await;
    let owner = McpClient::with_bearer(&server.base_url, &owner_key);
    owner
        .tools_call(
            "host.tool_publish",
            json!({"name": "rcpt", "kind": "echo", "spec": {"schema": {"type": "object", "required": ["msg"]}}}),
        )
        .await
        .expect("publish");
    owner
        .tools_call("host.tool_share", json!({"name": "rcpt", "visibility": "public", "description": "tdisc ac3 receipts"}))
        .await
        .expect("share");
    let full = format!("{owner_ns}.rcpt");

    let (plain_ns, plain_key) = signup_plain(&server.base_url, "Tdisc AC3 Plain Caller").await;
    let plain = McpClient::with_bearer(&server.base_url, &plain_key);
    let syn = signup_with_synthetic_header(&server.base_url, "Tdisc AC3 Synthetic Caller", "tdisc:ac3").await;
    let (syn_ns, syn_key) = (syn["tenant"].as_str().unwrap().to_string(), syn["key"].as_str().unwrap().to_string());
    let synthetic = McpClient::with_bearer(&server.base_url, &syn_key);

    // the owner's own call counts nowhere
    owner.tools_call(&full, json!({"msg": "own"})).await.expect("owner call");
    // the plain tenant: one ok, one error
    plain.tools_call(&full, json!({"msg": "ok"})).await.expect("ok call");
    plain.tools_call(&full, json!({})).await.expect_err("a call missing `msg` is an error");
    // the synthetic tenant: one ok
    synthetic.tools_call(&full, json!({"msg": "syn"})).await.expect("synthetic call");
    // one more plain call, aged to 31 days ago
    plain.tools_call(&full, json!({"msg": "old"})).await.expect("old call");
    let db = Connection::open(server.data_dir.0.join("mcphost.db")).unwrap();
    let old = mcphost::state::now_unix() - 31 * 86_400;
    let aged = db
        .execute(
            "UPDATE calls SET started_unix = ?1 WHERE id = (SELECT MAX(id) FROM calls WHERE tool_name = 'rcpt' AND ok = 1 \
             AND caller_tenant_id = (SELECT id FROM tenants WHERE namespace = ?2))",
            rusqlite::params![old, plain_ns],
        )
        .unwrap();
    assert_eq!(aged, 1);
    let rows: i64 = db.query_row("SELECT COUNT(*) FROM calls WHERE tool_name = 'rcpt'", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 5, "five recorded calls: owner, plain ok, plain error, synthetic, plain old");

    // a plain caller: its own class and the synthetic caller's calls are left out
    let (reader_ns, reader_key) = signup_plain(&server.base_url, "Tdisc AC3 Plain Reader").await;
    let reader = McpClient::with_bearer(&server.base_url, &reader_key);
    let plain_view = entry(&reader, &full).await;
    assert_eq!(plain_view["calls_30d"], json!({"n": 2, "ok": 1}), "{plain_view}");

    // a synthetic-marked caller sees every other tenant's calls
    let syn_view = entry(&synthetic, &full).await;
    assert_eq!(syn_view["calls_30d"], json!({"n": 3, "ok": 2}), "{syn_view}");

    // the override moves a plain caller to the synthetic count and back
    let found = extract_structured(
        &reader.tools_call("host.catalog.search", json!({"q": "tdisc ac3", "include_synthetic": true})).await.expect("search"),
    );
    assert_eq!(found["tools"][0]["calls_30d"], json!({"n": 3, "ok": 2}), "{found}");

    // the well-known document carries the plain count
    let doc: Value = reqwest::get(format!("{}/.well-known/mcp/catalog.json", server.base_url))
        .await
        .expect("GET")
        .json()
        .await
        .expect("json");
    assert_eq!(doc["tools"][0]["calls_30d"], json!({"n": 2, "ok": 1}), "{doc}");

    // no entry names a caller or a tenant id; the owner appears by namespace only
    for view in [&plain_view, &syn_view, &doc["tools"][0]] {
        let text = view.to_string();
        for ns in [&plain_ns, &syn_ns, &reader_ns] {
            assert!(!text.contains(ns.as_str()), "{ns} leaked into {text}");
        }
        assert_eq!(view["owner"], json!(owner_ns), "{text}");
        for key in view.as_object().unwrap().keys() {
            assert!(!key.contains("caller") && !key.contains("tenant"), "{key} in {text}");
        }
        assert!(!text.contains("tenant_id") && !text.contains("caller"), "{text}");
    }
}
