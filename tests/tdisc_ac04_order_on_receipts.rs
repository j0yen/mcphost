//! PRD-mcphost-trusted-discovery-catalog-receipts AC4 (P0) -- the catalog order
//! is verdict tier, smoothed ok ratio, call count, share date. Five plain
//! owners' tools: A pass 1/1, B pass 95/100, C untested with no calls, D
//! republished after a pass then tested will_fail with 100/100, E equal to C but
//! shared later. `catalog.search`, `tool_list_shared` and the well-known
//! document return B, A, E, C, D.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, extract_structured, publish, publish_forced, signup_plain};
use rusqlite::Connection;
use serde_json::{Value, json};

fn chain(mapping: &str) -> Value {
    json!({"steps": [
        {"tool": "step1", "args": {"leads": [{"name": "a"}]}},
        {"tool": "step2", "args": {"lead": mapping}},
    ]})
}

/// A public tool of its own plain owner, shared and so passed on version 1.
async fn owner_with_public(server: &TestServer, label: &str, kind: &str, spec: Value) -> (String, McpClient, String) {
    let (ns, key) = signup_plain(&server.base_url, label).await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let schema = json!({"type": "object"});
    publish(&client, "step1", "echo", json!({"schema": schema})).await;
    publish(&client, "step2", "echo", json!({"schema": schema})).await;
    let full = publish(&client, "ranked", kind, spec).await;
    client
        .tools_call("host.tool_share", json!({"name": "ranked", "visibility": "public", "description": "tdisc ac4 ranked"}))
        .await
        .expect("share");
    (ns, client, full)
}

fn seed_calls(db: &Connection, owner_ns: &str, caller_ns: &str, n: i64, ok: i64) {
    for i in 0..n {
        db.execute(
            "INSERT INTO calls (tenant_id, tool_name, started_at, started_unix, duration_ms, ok, caller_tenant_id) \
             VALUES ((SELECT id FROM tenants WHERE namespace = ?1), 'ranked', 'x', ?2, 1, ?3, \
                     (SELECT id FROM tenants WHERE namespace = ?4))",
            rusqlite::params![owner_ns, mcphost::state::now_unix() - 60, (i < ok) as i64, caller_ns],
        )
        .unwrap();
    }
}

fn names(listing: &Value) -> Vec<String> {
    listing["tools"].as_array().expect("tools").iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn tdisc_ac04() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let good = chain("$.prev.result.leads");
    let bad = chain("$.prev.result.leadz");
    let echo = json!({"schema": {"type": "object"}});

    let (a_ns, _a, a) = owner_with_public(&server, "Tdisc AC4 A", "echo", echo.clone()).await;
    let (b_ns, _b, b) = owner_with_public(&server, "Tdisc AC4 B", "echo", echo.clone()).await;
    let (c_ns, c_client, c) = owner_with_public(&server, "Tdisc AC4 C", "echo", echo.clone()).await;
    let (d_ns, d_client, d) = owner_with_public(&server, "Tdisc AC4 D", "chain", good).await;
    let (e_ns, e_client, e) = owner_with_public(&server, "Tdisc AC4 E", "echo", echo.clone()).await;

    // C and E: republished after their pass, never retested -> untested.
    for client in [&c_client, &e_client] {
        publish(client, "ranked", "echo", json!({"schema": {"type": "object"}, "description": "v2"})).await;
    }
    // D: republished after a pass, then tested will_fail on the current version.
    publish_forced(&d_client, "ranked", "chain", bad).await;
    let tested = extract_structured(&d_client.tools_call("host.tool_test", json!({"name": "ranked", "args": {}})).await.unwrap());
    assert_eq!(tested["verdict"], json!("will_fail"), "{tested}");

    // calls by plain tenants, seeded straight into the database
    let (caller_ns, _k) = signup_plain(&server.base_url, "Tdisc AC4 Caller").await;
    let db = Connection::open(server.data_dir.0.join("mcphost.db")).unwrap();
    seed_calls(&db, &a_ns, &caller_ns, 1, 1);
    seed_calls(&db, &b_ns, &caller_ns, 100, 95);
    seed_calls(&db, &d_ns, &caller_ns, 100, 100);
    // share dates: C before E, the rest in between (the SQL ties on whole seconds otherwise)
    for (i, ns) in [&c_ns, &a_ns, &b_ns, &d_ns, &e_ns].iter().enumerate() {
        db.execute(
            "UPDATE tools SET shared_unix = ?1 WHERE tenant_id = (SELECT id FROM tenants WHERE namespace = ?2)",
            rusqlite::params![1_000_000 + i as i64, ns],
        )
        .unwrap();
    }

    let want = vec![b, a, e, c, d];
    let (_s_ns, s_key) = signup_plain(&server.base_url, "Tdisc AC4 Sixth").await;
    let sixth = McpClient::with_bearer(&server.base_url, &s_key);
    let search = extract_structured(&sixth.tools_call("host.catalog.search", json!({"q": "tdisc ac4"})).await.unwrap());
    assert_eq!(names(&search), want, "catalog.search: {search}");
    let shared = extract_structured(&sixth.tools_call("host.tool_list_shared", json!({"q": "tdisc ac4"})).await.unwrap());
    assert_eq!(names(&shared), want, "tool_list_shared: {shared}");
    let doc: Value = reqwest::get(format!("{}/.well-known/mcp/catalog.json", server.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(names(&doc), want, "well-known: {doc}");

    // limit applies after the order
    let top = extract_structured(&sixth.tools_call("host.catalog.search", json!({"q": "tdisc ac4", "limit": 2})).await.unwrap());
    assert_eq!(names(&top), want[..2].to_vec(), "{top}");
    // the tiers and counts behind the order are on the entries
    let by_name = |n: &str| search["tools"].as_array().unwrap().iter().find(|t| t["name"] == json!(n)).unwrap().clone();
    assert_eq!(by_name(&want[0])["test_verdict"], json!("pass"));
    assert_eq!(by_name(&want[0])["calls_30d"], json!({"n": 100, "ok": 95}));
    assert_eq!(by_name(&want[2])["test_verdict"], Value::Null);
    assert_eq!(by_name(&want[4])["test_verdict"], json!("will_fail"));
}
