//! PRD-mcphost-index-refresh-on-change
//! AC2 -- Given an index over a 100-row table and 8 documents, When 20 rows
//! are appended and one document is re-put, Then before the tick
//! `host.index.status` shows `behind_by: 20` and `behind_by: 1`, and after
//! one tick both read 0 with chunk counts updated.

use crate::common;
use mcphost::{docs, docs_index, index, tables};
use serde_json::{Value, json};

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hrfr-ac02-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn source<'a>(status: &'a Value, kind: &str) -> &'a Value {
    status["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .find(|s| s["kind"] == json!(kind))
        .unwrap_or_else(|| panic!("no {kind} source in {status}"))
}

#[tokio::test]
async fn appended_rows_and_a_reput_document_read_behind_then_zero_after_one_tick() {
    let dir = scratch_dir();
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hrfr-ac02").await;

    for n in 0..8 {
        docs::doc_put(&state, &tenant, &json!({"name": format!("hb/doc{n}.md"), "content": format!("handbook page {n}")}))
            .await
            .expect("put ok");
    }
    tables::table_create(&state, &tenant, &json!({"name": "orders", "columns": {"id": "integer", "notes": "text"}}))
        .await
        .expect("table create");
    let rows: Vec<_> = (0..100).map(|i| json!({"id": i, "notes": format!("order {i} wants gift wrap")})).collect();
    tables::table_append(&state, &tenant, &json!({"table": "orders", "rows": rows})).await.expect("append");

    index::index_create(
        &state,
        &tenant,
        &json!({"name": "handbook", "sources": [
            {"kind": "doc", "prefix": "hb/"},
            {"kind": "table", "table": "orders", "columns": ["notes"]},
        ]}),
    )
    .await
    .expect("create ok");
    docs_index::tick_once(&state).await.expect("tick 1");
    docs_index::tick_once(&state).await.expect("tick 2");

    let built = index::index_status(&state, &tenant, &json!({"name": "handbook"})).await.expect("status");
    assert_eq!(built["pending"], json!(0), "{built}");
    assert_eq!(source(&built, "doc")["behind_by"], json!(0), "{built}");
    assert_eq!(source(&built, "table")["behind_by"], json!(0), "{built}");
    let chunks_before = built["chunks"].as_i64().unwrap();
    assert_eq!(chunks_before, 108, "8 documents and 100 rows, one chunk each: {built}");

    let more: Vec<_> = (100..120).map(|i| json!({"id": i, "notes": format!("order {i} wants gift wrap")})).collect();
    tables::table_append(&state, &tenant, &json!({"table": "orders", "rows": more})).await.expect("append 20");
    docs::doc_put(&state, &tenant, &json!({"name": "hb/doc3.md", "content": "handbook page 3, revised"}))
        .await
        .expect("re-put");

    let behind = index::index_status(&state, &tenant, &json!({"name": "handbook"})).await.expect("status");
    assert_eq!(source(&behind, "table")["behind_by"], json!(20), "{behind}");
    assert_eq!(source(&behind, "doc")["behind_by"], json!(1), "{behind}");

    docs_index::tick_once(&state).await.expect("refresh tick");

    let after = index::index_status(&state, &tenant, &json!({"name": "handbook"})).await.expect("status");
    assert_eq!(source(&after, "table")["behind_by"], json!(0), "{after}");
    assert_eq!(source(&after, "doc")["behind_by"], json!(0), "{after}");
    assert_eq!(after["chunks"], json!(chunks_before + 20), "20 new row chunks, the re-put document re-chunked in place: {after}");
    assert_eq!(source(&after, "table")["items"], json!(120), "{after}");
    assert_eq!(source(&after, "table")["chunks"], json!(120), "{after}");
    assert_eq!(source(&after, "doc")["chunks"], json!(8), "{after}");

    let conn = rusqlite::Connection::open(dir.join("mcphost.db")).expect("open db");
    let revised: i64 = conn
        .query_row("SELECT COUNT(*) FROM index_chunks WHERE text LIKE '%revised%'", [], |r| r.get(0))
        .expect("count");
    assert_eq!(revised, 1, "the re-put text replaced the old chunk");

    std::fs::remove_dir_all(&dir).ok();
}
