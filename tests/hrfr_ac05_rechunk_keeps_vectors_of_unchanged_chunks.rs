//! PRD-mcphost-index-refresh-on-change
//! AC5 -- Given a document re-put with one paragraph changed and vectors
//! present, When the tick re-chunks it, Then only the chunks whose text
//! changed have `vector IS NULL` and the fixture provider saw exactly that
//! many inputs.
//!
//! The provider answers the build, then fails (503) for the refresh: the
//! changed chunk is sent to it (its input is counted) and, with no answer,
//! stays lexical-only (`vector IS NULL`), while every unchanged chunk keeps
//! the vector it already had without a provider call.

use crate::common;
use mcphost::{docs, index};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hrfr-ac05-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Three ~700-byte paragraphs, so the default 1200-byte window gives one
/// chunk per paragraph. `second` is the head of the middle paragraph; the
/// overlap a chunk carries is the previous chunk's *tail*, so changing a
/// head changes exactly one chunk's text.
fn policy_text(second: &str) -> String {
    ["alpha", second, "gamma"]
        .iter()
        .map(|head| format!("{head} {}", "w".repeat(700)))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn vector_counts(dir: &std::path::Path) -> (i64, i64) {
    let conn = rusqlite::Connection::open(dir.join("mcphost.db")).expect("open db");
    conn.query_row(
        "SELECT COALESCE(SUM(vector IS NULL), 0), COALESCE(SUM(vector IS NOT NULL), 0) FROM index_chunks",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .expect("counts")
}

#[tokio::test]
async fn only_the_changed_chunk_loses_its_vector_and_only_it_reaches_the_provider() {
    let dir = scratch_dir();
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hrfr-ac05").await;

    let fail = Arc::new(AtomicBool::new(false));
    let seen: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let provider = MockServer::start().await;
    {
        let (fail, seen) = (fail.clone(), seen.clone());
        Mock::given(method("POST"))
            .respond_with(move |req: &Request| {
                let body: serde_json::Value = serde_json::from_slice(&req.body).expect("json body");
                let inputs = body["input"].as_array().expect("input array");
                seen.lock().unwrap().push(inputs.len());
                if fail.load(Ordering::SeqCst) {
                    return ResponseTemplate::new(503);
                }
                let data: Vec<_> = inputs.iter().map(|_| json!({"embedding": [0.5, 0.5]})).collect();
                ResponseTemplate::new(200).set_body_json(json!({"data": data}))
            })
            .mount(&provider)
            .await;
    }
    let (ct, nonce) = state.secrets.encrypt("test-embed-secret-value").unwrap();
    state.db.upsert_secret(tenant.id, "EMBED_KEY".to_string(), ct, nonce).await.expect("secret");
    docs::doc_index_config(
        &state,
        &tenant,
        &json!({
            "provider": "openai-compatible",
            "endpoint": format!("{}/v1/embeddings", provider.uri()),
            "model": "m",
            "secret": "EMBED_KEY",
        }),
    )
    .await
    .expect("index_config ok");

    docs::doc_put(&state, &tenant, &json!({"name": "hb/policy.md", "content": policy_text("beta")}))
        .await
        .expect("put");
    index::index_create(&state, &tenant, &json!({"name": "handbook", "sources": [{"kind": "doc", "prefix": "hb/"}]}))
        .await
        .expect("create");
    // The index pass alone: the documents indexer would call the same provider.
    index::tick_tenant(&state, tenant.id).await.expect("build tick");

    let (nulls, vectors) = vector_counts(&dir);
    assert_eq!(nulls, 0, "every chunk was embedded at build time");
    assert!(vectors >= 3, "one chunk per paragraph at least: {vectors}");
    assert_eq!(*seen.lock().unwrap(), vec![vectors as usize], "the build embedded each chunk once");

    fail.store(true, Ordering::SeqCst);
    docs::doc_put(&state, &tenant, &json!({"name": "hb/policy.md", "content": policy_text("BETA CHANGED")}))
        .await
        .expect("re-put");
    index::tick_tenant(&state, tenant.id).await.expect("refresh tick");

    let (nulls, kept) = vector_counts(&dir);
    assert_eq!(nulls, 1, "exactly the changed chunk has no vector");
    assert_eq!(kept, vectors - 1, "every unchanged chunk kept its vector");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![vectors as usize, 1],
        "the provider saw the one changed chunk and nothing else"
    );

    std::fs::remove_dir_all(&dir).ok();
}
