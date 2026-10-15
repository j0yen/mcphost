//! PRD-mcphost-index-create-from-sources
//! AC2 -- Given 8 documents under `hb/`, a 100-row table with a `notes`
//! column and a fixture URL, When `host.index.create` runs with the three
//! sources and two ticks pass, Then `host.index.status` reports
//! `pending: 0`, `chunks` >= 110, three source rows each with `items` > 0,
//! and every chunk's `byte_end - byte_start` equals its `text` length in
//! bytes.

use crate::common;
use mcphost::{docs, docs_index, index, tables};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hidx-ac02-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Three ~700-byte paragraphs: with the default 1200-byte window each
/// document chunks into more than one piece, a multi-byte character
/// included so byte (not char) offsets are what the test pins.
fn handbook_text(n: usize) -> String {
    (0..3)
        .map(|p| format!("handbook {n} section {p} caf\u{e9} {}", "w".repeat(700)))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[tokio::test]
async fn three_sources_build_in_two_ticks_with_byte_exact_offsets() {
    let dir = scratch_dir();
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hidx-ac02").await;

    for n in 0..8 {
        docs::doc_put(&state, &tenant, &json!({"name": format!("hb/doc{n}.md"), "content": handbook_text(n)}))
            .await
            .expect("put ok");
    }
    docs::doc_put(&state, &tenant, &json!({"name": "other/skip.md", "content": "not under hb"}))
        .await
        .expect("put ok");

    tables::table_create(&state, &tenant, &json!({"name": "orders", "columns": {"id": "integer", "notes": "text"}}))
        .await
        .expect("table create");
    let rows: Vec<_> = (0..100).map(|i| json!({"id": i, "notes": format!("order {i} wants gift wrap")})).collect();
    tables::table_append(&state, &tenant, &json!({"table": "orders", "rows": rows}))
        .await
        .expect("table append");

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/faq"))
        .respond_with(ResponseTemplate::new(200).set_body_string("Refunds are processed within 14 days."))
        .mount(&server)
        .await;
    let url = format!("{}/faq", server.uri());

    let created = index::index_create(
        &state,
        &tenant,
        &json!({"name": "handbook", "sources": [
            {"kind": "doc", "prefix": "hb/"},
            {"kind": "table", "table": "orders", "columns": ["notes"]},
            {"kind": "url", "url": url},
        ]}),
    )
    .await
    .expect("create ok");
    assert_eq!(created, json!({"name": "handbook", "sources": 3, "status": "building"}));

    let before = index::index_status(&state, &tenant, &json!({"name": "handbook"})).await.expect("status");
    assert_eq!(before["pending"], json!(3), "nothing builds until a tick runs: {before}");

    docs_index::tick_once(&state).await.expect("tick 1");
    docs_index::tick_once(&state).await.expect("tick 2");

    let status = index::index_status(&state, &tenant, &json!({"name": "handbook"})).await.expect("status");
    assert_eq!(status["pending"], json!(0), "{status}");
    assert!(status["chunks"].as_i64().unwrap() >= 110, "{status}");
    let sources = status["sources"].as_array().expect("sources");
    assert_eq!(sources.len(), 3, "{status}");
    for s in sources {
        assert!(s["items"].as_i64().unwrap() > 0, "every source indexed items: {s}");
        assert!(s.get("last_error").is_none(), "no source errored: {s}");
    }
    assert_eq!(sources[0]["items"], json!(8), "only the hb/ documents: {status}");
    assert_eq!(sources[1]["items"], json!(100), "{status}");
    assert_eq!(sources[2]["items"], json!(1), "{status}");

    let conn = rusqlite::Connection::open(dir.join("mcphost.db")).expect("open db");
    let mut stmt = conn.prepare("SELECT byte_start, byte_end, text FROM index_chunks").expect("prepare");
    let rows: Vec<(i64, i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert_eq!(rows.len() as i64, status["chunks"].as_i64().unwrap());
    assert!(rows.iter().any(|(_, _, t)| t.contains('\u{e9}')), "multi-byte text was indexed");
    for (start, end, text) in &rows {
        assert_eq!(end - start, text.len() as i64, "byte_end - byte_start must be the text's byte length");
    }

    std::fs::remove_dir_all(&dir).ok();
}
