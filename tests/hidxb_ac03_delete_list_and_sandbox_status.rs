//! PRD-mcphost-index-create-from-sources-b
//! AC3 -- Given two indexes, When `host.index.delete` removes one, Then
//! `host.index.list` shows one and `index_chunks` holds no rows for the
//! deleted id; Given the sandbox, When a tool calls
//! `mcphost.index.status(name=...)`, Then it returns the same JSON as the
//! host verb.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::{docs, docs_index, index, sandbox};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn deleting_one_of_two_indexes_leaves_one_and_no_orphan_chunks() {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hidxb-ac03-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hidxb-ac03").await;
    for n in 0..3 {
        docs::doc_put(&state, &tenant, &json!({"name": format!("hb/{n}.md"), "content": format!("handbook page {n}")}))
            .await
            .expect("put ok");
        docs::doc_put(&state, &tenant, &json!({"name": format!("faq/{n}.md"), "content": format!("faq page {n}")}))
            .await
            .expect("put ok");
    }
    for (name, prefix) in [("handbook", "hb/"), ("faq", "faq/")] {
        index::index_create(&state, &tenant, &json!({"name": name, "sources": [{"kind": "doc", "prefix": prefix}]}))
            .await
            .expect("create ok");
    }
    docs_index::tick_once(&state).await.expect("tick");

    let listed = index::index_list(&state, &tenant, &json!({})).await.expect("list");
    let listed = listed["indexes"].clone();
    let names: Vec<&str> = listed.as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["handbook", "faq"], "{listed}");
    assert_eq!(listed[0]["chunks"], json!(3), "{listed}");
    assert_eq!(listed[0]["sources"], json!(1), "{listed}");

    let conn = rusqlite::Connection::open(dir.join("mcphost.db")).expect("open db");
    let gone_id: i64 =
        conn.query_row("SELECT id FROM indexes WHERE name = 'handbook'", [], |r| r.get(0)).expect("index id");
    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM index_chunks WHERE index_id = ?1", [gone_id], |r| r.get(0))
        .expect("count");
    assert!(before > 0, "the index had chunks to remove");

    let deleted = index::index_delete(&state, &tenant, &json!({"name": "handbook"})).await.expect("delete");
    assert_eq!(deleted["chunks_removed"], json!(before), "{deleted}");

    let listed = index::index_list(&state, &tenant, &json!({})).await.expect("list")["indexes"].clone();
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed[0]["name"], json!("faq"));
    for table in ["index_chunks", "index_sources"] {
        let col = "index_id";
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table} WHERE {col} = ?1"), [gone_id], |r| r.get(0))
            .expect("count");
        assert_eq!(n, 0, "{table} holds no rows for the deleted id");
    }
    let fts: i64 = conn
        .query_row("SELECT COUNT(*) FROM index_chunks_fts WHERE index_chunks_fts MATCH 'handbook'", [], |r| r.get(0))
        .expect("fts count");
    assert_eq!(fts, 0, "the full-text rows went with the chunks");
    let kept: i64 = conn.query_row("SELECT COUNT(*) FROM index_chunks", [], |r| r.get(0)).expect("count");
    assert_eq!(kept, 3, "the other index's chunks are untouched");

    let err = index::index_delete(&state, &tenant, &json!({"name": "handbook"})).await.expect_err("already gone");
    assert_eq!(err.code(), "index_not_found");
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn host_verbs_list_and_delete_work_over_mcp() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "Hidxb AC3 Verbs").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for name in ["one", "two"] {
        client
            .tools_call("host.index.create", json!({"name": name, "sources": [{"kind": "doc", "prefix": "x/"}]}))
            .await
            .expect("create");
    }
    let listed = extract_structured(&client.tools_call("host.index.list", json!({})).await.expect("list"));
    assert_eq!(listed["indexes"].as_array().map(Vec::len), Some(2), "{listed}");
    client.tools_call("host.index.delete", json!({"name": "one"})).await.expect("delete");
    let listed = extract_structured(&client.tools_call("host.index.list", json!({})).await.expect("list"));
    assert_eq!(listed["indexes"].as_array().map(Vec::len), Some(1), "{listed}");
}

#[tokio::test]
async fn sandbox_index_status_returns_the_same_json_as_the_host_verb() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "Hidxb AC3 Sandbox").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    client
        .tools_call("host.docs.put", json!({"name": "hb/a.md", "content": "handbook page about refunds"}))
        .await
        .expect("put");
    client
        .tools_call("host.index.create", json!({"name": "handbook", "sources": [{"kind": "doc", "prefix": "hb/"}]}))
        .await
        .expect("create");
    // Let the scheduler finish so lag_seconds is a stable 0 in both reads.
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    let host_status = loop {
        let s = extract_structured(
            &client.tools_call("host.index.status", json!({"name": "handbook"})).await.expect("status"),
        );
        if s["pending"] == json!(0) {
            break s;
        }
        assert!(std::time::Instant::now() < deadline, "index never finished building: {s}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    assert!(host_status["chunks"].as_i64().unwrap() >= 1, "{host_status}");

    let spec = json!({
        "network": "none",
        "source": "import mcphost\ndef main(args):\n    return mcphost.index.status(name=args['name'])\n",
        "args_schema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]},
    });
    client
        .tools_call("host.tool_publish", json!({"name": "idx_status", "kind": "python", "spec": spec}))
        .await
        .expect("publish");
    let result = poll_until_ready(
        &client,
        &format!("{ns}.idx_status"),
        json!({"name": "handbook"}),
        Duration::from_secs(15),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    assert_eq!(extract_structured(&result), host_status, "the sandbox returns the host verb's JSON");
}
