//! PRD-mcphost-index-refresh-on-change
//! AC7 -- Given a URL failing three ticks in a row, When the fourth tick
//! runs, Then the source is `paused: true` and not fetched; When
//! `host.index.refresh` names it, Then the next tick fetches it.

use crate::common;
use mcphost::{docs_index, index};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hrfr-ac07-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn three_failures_pause_the_source_and_a_refresh_resumes_it() {
    let dir = scratch_dir();
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hrfr-ac07").await;

    let hits = Arc::new(AtomicUsize::new(0));
    let healthy = Arc::new(AtomicBool::new(false));
    let server = MockServer::start().await;
    {
        let (hits, healthy) = (hits.clone(), healthy.clone());
        Mock::given(method("GET"))
            .respond_with(move |_: &Request| {
                hits.fetch_add(1, Ordering::SeqCst);
                if healthy.load(Ordering::SeqCst) {
                    ResponseTemplate::new(200).set_body_string("Refunds take 14 days.")
                } else {
                    ResponseTemplate::new(500)
                }
            })
            .mount(&server)
            .await;
    }
    let url = format!("{}/faq", server.uri());
    index::index_create(&state, &tenant, &json!({"name": "faq", "sources": [{"kind": "url", "url": url}]}))
        .await
        .expect("create ok");
    let status = || async { index::index_status(&state, &tenant, &json!({"name": "faq"})).await.expect("status") };

    for n in 1..=3 {
        docs_index::tick_once(&state).await.expect("failing tick");
        assert_eq!(hits.load(Ordering::SeqCst), n, "tick {n} fetched the failing url");
    }
    let paused = status().await;
    assert_eq!(paused["sources"][0]["paused"], json!(true), "{paused}");
    assert_eq!(paused["sources"][0]["last_error"], json!("http_500"), "{paused}");

    docs_index::tick_once(&state).await.expect("fourth tick");
    assert_eq!(hits.load(Ordering::SeqCst), 3, "a paused source is not fetched");

    healthy.store(true, Ordering::SeqCst);
    docs_index::tick_once(&state).await.expect("still paused");
    assert_eq!(hits.load(Ordering::SeqCst), 3, "a healthy server alone does not resume it");

    let queued = index::index_refresh(&state, &tenant, &json!({"name": "faq", "source": url})).await.expect("refresh");
    assert_eq!(queued, json!({"queued": 1}));
    docs_index::tick_once(&state).await.expect("tick after refresh");
    assert_eq!(hits.load(Ordering::SeqCst), 4, "the forced refresh is fetched on the next tick");

    let resumed = status().await;
    let source = &resumed["sources"][0];
    assert!(source.get("paused").is_none(), "{resumed}");
    assert!(source.get("last_error").is_none(), "R6: the first clean pass clears the error: {resumed}");
    assert_eq!(source["chunks"], json!(1), "{resumed}");
    assert_eq!(source["behind_by"], json!(0), "{resumed}");

    let unknown = index::index_refresh(&state, &tenant, &json!({"name": "faq", "source": "http://nowhere/"})).await;
    assert!(unknown.is_err(), "a ref that names no source is refused");

    std::fs::remove_dir_all(&dir).ok();
}
