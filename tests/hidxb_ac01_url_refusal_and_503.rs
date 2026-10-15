//! PRD-mcphost-index-create-from-sources-b
//! AC1 -- Given a URL source pointing at `https://mcphost.dev/mcp` or
//! `http://10.0.0.1/`, When `host.index.create` runs, Then `host_not_allowed`
//! is returned at create time and no row is written; Given a URL that
//! answers 503, Then the source row carries `last_error` and the other
//! sources still build.

use crate::common;
use mcphost::{docs, docs_index, index};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn scratch_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hidxb-ac01-{tag}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn own_domain_and_private_urls_are_refused_at_create_and_write_no_row() {
    let dir = scratch_dir("refuse");
    let mut state = common::bare_state(&dir).await;
    // A real deployment: the instance's own domain is its public url.
    state.public_url = "https://mcphost.dev".to_string();
    let tenant = common::bare_tenant(&state, "hidxb-ac01-refuse").await;

    for url in ["https://mcphost.dev/mcp", "http://10.0.0.1/"] {
        let err = index::index_create(
            &state,
            &tenant,
            &json!({"name": "faq", "sources": [
                {"kind": "doc", "prefix": "hb/"},
                {"kind": "url", "url": url},
            ]}),
        )
        .await
        .expect_err("a disallowed host is refused at create time");
        assert_eq!(err.code(), "host_not_allowed", "{url}: {err:?}");
    }

    let conn = rusqlite::Connection::open(dir.join("mcphost.db")).expect("open db");
    for table in ["indexes", "index_sources", "index_chunks"] {
        let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).expect("count");
        assert_eq!(n, 0, "no row written to {table}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_503_url_records_last_error_and_the_other_sources_still_build() {
    let dir = scratch_dir("503");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hidxb-ac01-503").await;
    docs::doc_put(&state, &tenant, &json!({"name": "hb/a.md", "content": "handbook page about refunds"}))
        .await
        .expect("put ok");

    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/down")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
    let url = format!("{}/down", server.uri());

    index::index_create(
        &state,
        &tenant,
        &json!({"name": "faq", "sources": [
            {"kind": "url", "url": url},
            {"kind": "doc", "prefix": "hb/"},
        ]}),
    )
    .await
    .expect("create ok");

    docs_index::tick_once(&state).await.expect("tick");

    let status = index::index_status(&state, &tenant, &json!({"name": "faq"})).await.expect("status");
    assert_eq!(status["pending"], json!(0), "{status}");
    let sources = status["sources"].as_array().expect("sources");
    assert_eq!(sources[0]["kind"], json!("url"));
    assert_eq!(sources[0]["last_error"], json!("http_503"), "{status}");
    assert_eq!(sources[0]["chunks"], json!(0), "{status}");
    assert!(sources[1].get("last_error").is_none(), "the doc source is clean: {status}");
    assert!(sources[1]["chunks"].as_i64().unwrap() >= 1, "the doc source built: {status}");

    std::fs::remove_dir_all(&dir).ok();
}
