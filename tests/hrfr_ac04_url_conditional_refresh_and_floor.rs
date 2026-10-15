//! PRD-mcphost-index-refresh-on-change
//! AC4 -- Given a fixture URL that answers 304 then a changed body, When two
//! ticks run past `refresh_s`, Then the first leaves chunks unchanged and the
//! second replaces them; Given `refresh_s: 60` on a free tenant, Then
//! `args_invalid` carries `data.minimum: 300`.

use crate::common;
use mcphost::errors::AppError;
use mcphost::{docs_index, index};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hrfr-ac04-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn chunk_rows(dir: &std::path::Path) -> Vec<(i64, String)> {
    let conn = rusqlite::Connection::open(dir.join("mcphost.db")).expect("open db");
    let mut stmt = conn.prepare("SELECT id, text FROM index_chunks ORDER BY id").expect("prepare");
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

/// Hit 0 builds (200, ETag v1); hit 1 must carry `If-None-Match: "v1"` and
/// gets a 304; hit 2 carries it again and gets the changed body (ETag v2).
fn faq_server_responder(hits: std::sync::Arc<AtomicUsize>) -> impl Fn(&Request) -> ResponseTemplate {
    move |req: &Request| {
        let n = hits.fetch_add(1, Ordering::SeqCst);
        let conditional = req.headers.get("if-none-match").and_then(|v| v.to_str().ok()) == Some("\"v1\"");
        match n {
            0 => ResponseTemplate::new(200).insert_header("ETag", "\"v1\"").set_body_string("Refunds take 14 days."),
            1 if conditional => ResponseTemplate::new(304),
            2 if conditional => ResponseTemplate::new(200)
                .insert_header("ETag", "\"v2\"")
                .set_body_string("Refunds now take 3 days."),
            _ => ResponseTemplate::new(500),
        }
    }
}

#[tokio::test]
async fn a_304_leaves_the_chunks_and_a_changed_body_replaces_them() {
    let dir = scratch_dir("conditional");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hrfr-ac04").await;

    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let server = MockServer::start().await;
    Mock::given(method("GET")).respond_with(faq_server_responder(hits.clone())).mount(&server).await;
    let url = format!("{}/faq", server.uri());

    index::index_create(
        &state,
        &tenant,
        &json!({"name": "faq", "sources": [{"kind": "url", "url": url, "refresh_s": 300}]}),
    )
    .await
    .expect("create ok");
    docs_index::tick_once(&state).await.expect("build tick");
    let built = chunk_rows(&dir);
    assert_eq!(built.len(), 1, "{built:?}");
    assert!(built[0].1.contains("14 days"));

    // A tick before `refresh_s` has passed does not fetch at all.
    docs_index::tick_once(&state).await.expect("early tick");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "not due yet, so not fetched");

    let conn = rusqlite::Connection::open(dir.join("mcphost.db")).expect("open db");
    let age = |conn: &rusqlite::Connection| {
        conn.execute("UPDATE index_sources SET last_checked_at = 0", []).expect("age the last check");
    };

    age(&conn);
    docs_index::tick_once(&state).await.expect("304 tick");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert_eq!(chunk_rows(&dir), built, "a 304 leaves the chunks exactly as they were");
    let status = index::index_status(&state, &tenant, &json!({"name": "faq"})).await.expect("status");
    assert_eq!(status["sources"][0]["behind_by"], json!(0), "{status}");

    age(&conn);
    docs_index::tick_once(&state).await.expect("changed tick");
    assert_eq!(hits.load(Ordering::SeqCst), 3);
    let replaced = chunk_rows(&dir);
    assert_eq!(replaced.len(), 1, "{replaced:?}");
    assert!(replaced[0].1.contains("3 days"), "the changed body replaced the chunk: {replaced:?}");
    assert_ne!(replaced[0].0, built[0].0);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn refresh_s_under_the_free_floor_is_args_invalid_with_the_minimum() {
    let dir = scratch_dir("floor");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hrfr-ac04-floor").await;

    let err = index::index_create(
        &state,
        &tenant,
        &json!({"name": "faq", "sources": [{"kind": "url", "url": "http://127.0.0.1:1/faq", "refresh_s": 60}]}),
    )
    .await
    .expect_err("60 s is under the free plan's floor");
    match err {
        AppError::Structured { code, data, .. } => {
            assert_eq!(code, "args_invalid");
            assert_eq!(data["minimum"], json!(300), "{data}");
        }
        other => panic!("expected a structured args_invalid, got {other:?}"),
    }

    std::fs::remove_dir_all(&dir).ok();
}
