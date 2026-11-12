//! PRD-mcphost-tool-reach-manifest-v2
//! AC4 (P0) -- Given any published tool, When `host.tool.reach` is called,
//! Then the answer equals `reach::derive` over the row's stored `kind` and
//! `spec` and the read writes nothing to the database; Given it is republished
//! with a new url host, Then `hosts` follows and `tool_version` increments, and
//! after `host.tool.rollback` the earlier manifest returns.
//!
//! "Writes nothing" is read off a second SQLite connection to the server's
//! database: every row of every table is dumped before and after the reads and
//! compared. The one column left out is `last_seen_unix`, which the auth layer
//! touches on every authenticated call of any verb.

use std::time::Duration;

use crate::common;
use common::{McpClient, TestServer, extract_structured, http_kind_registry, signup};
use serde_json::{Value, json};

fn http_spec(host: &str) -> Value {
    json!({"method": "GET", "url": format!("https://{host}/v1/balance"),
           "headers": {"Authorization": "Bearer {{ secret.stripe }}"}})
}

async fn reach(client: &McpClient, name: &str) -> Value {
    let result = client
        .tools_call("host.tool.reach", json!({"name": name}))
        .await
        .unwrap_or_else(|e| panic!("host.tool.reach {name}: {} {}", e.code, e.message));
    extract_structured(&result)
}


/// Every row of every table, as text, keyed by table name; `last_seen_unix` is left out.
fn dump(c: &rusqlite::Connection) -> std::collections::BTreeMap<String, Vec<String>> {
    let names: Vec<String> = c
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut out = std::collections::BTreeMap::new();
    for name in names {
        let mut stmt = c.prepare(&format!("SELECT * FROM \"{name}\"")).unwrap();
        let kept: Vec<usize> =
            stmt.column_names().enumerate().filter(|(_, n)| *n != "last_seen_unix").map(|(i, _)| i).collect();
        let rows = stmt
            .query_map([], |r| Ok(kept.iter().map(|i| format!("{:?}", r.get_ref(*i).unwrap())).collect::<Vec<_>>().join("|")))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        out.insert(name, rows);
    }
    out
}

#[tokio::test]
async fn trm_ac04_the_answer_is_derive_over_the_stored_row_and_follows_versions() {
    let server = TestServer::start_with_kinds(http_kind_registry()).await;
    let (ns, key) = signup(&server.base_url, "TRM AC4 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    let tenant = server.state.db.find_tenant_by_namespace(ns.clone()).await.expect("db").expect("tenant");
    client.tools_call("host.secret.set", json!({"name": "stripe", "value": "trm-ac4"})).await.expect("secret");

    let publish = |spec: Value| {
        let client = &client;
        async move {
            client
                .tools_call("host.tool.publish", json!({"name": "shop", "kind": "http", "spec": spec}))
                .await
                .unwrap_or_else(|e| panic!("publish: {} {}", e.code, e.message));
        }
    };
    let stored = |name: &'static str| {
        let (db, id) = (server.state.db.clone(), tenant.id);
        async move { db.get_tool(id, name.to_string()).await.expect("db").expect("row") }
    };

    publish(http_spec("old.example.com")).await;
    client
        .tools_call("host.tool.publish", json!({"name": "plain", "kind": "echo", "spec": {"schema": {"type": "object"}}}))
        .await
        .expect("publish echo");

    // The answer is derive over the stored kind and spec, for more than one kind.
    for name in ["shop", "plain"] {
        let row = stored(name).await;
        let answer = reach(&client, name).await;
        assert_eq!(answer["reach"], mcphost::reach::derive(&row.kind, &row.spec).to_json(), "{name}: {answer}");
        assert_eq!(answer["kind"], json!(row.kind));
        assert_eq!(answer["tool_version"], json!(row.current_version));
    }

    // The read writes nothing: no other connection's commit is seen by a probe.
    let probe = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("probe connection");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let rows_before = dump(&probe);
    for _ in 0..3 {
        reach(&client, "shop").await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let rows_after = dump(&probe);
    for (table, rows) in &rows_before {
        assert_eq!(rows, &rows_after[table], "host.tool.reach changed table {table}");
    }
    assert_eq!(rows_before.keys().collect::<Vec<_>>(), rows_after.keys().collect::<Vec<_>>());

    // Republish with a new host: hosts follows, the version increments.
    let first = reach(&client, "shop").await;
    assert_eq!(first["reach"]["hosts"], json!(["old.example.com"]));
    assert_eq!(first["tool_version"], json!(1));
    publish(http_spec("new.example.com")).await;
    let second = reach(&client, "shop").await;
    assert_eq!(second["reach"]["hosts"], json!(["new.example.com"]));
    assert_eq!(second["tool_version"], json!(2));

    // Rollback brings the earlier manifest back, the version with it.
    client.tools_call("host.tool.rollback", json!({"name": "shop", "version": 1})).await.expect("rollback");
    assert_eq!(reach(&client, "shop").await, first);
}
