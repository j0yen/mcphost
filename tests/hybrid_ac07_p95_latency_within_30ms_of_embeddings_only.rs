//! PRD-mcphost-docs-hybrid-search
//! AC7 -- Given a tenant with 1,000 chunks, When 50 hybrid searches run,
//! Then p95 latency exceeds embeddings-only p95 by at most 30 ms.
//!
//! Seeded in one transaction via
//! [`mcphost::db::Db::doc_chunks_bulk_insert_with_vectors_for_test`],
//! bypassing `docs::doc_put`/`docs_index::tick_once` entirely -- same "the
//! AC is about search latency against a populated index, not about how
//! fast this test can build it" shape
//! `docsearch_ac11_lexical_search_p95_latency.rs`'s own
//! `doc_chunks_bulk_insert_for_test` already uses.

use crate::common;
use mcphost::docs;
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const DIMS: usize = 64;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hybrid-ac07-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn p95(mut durations: Vec<std::time::Duration>) -> std::time::Duration {
    durations.sort();
    let idx = ((durations.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
    durations[idx]
}

#[tokio::test]
async fn hybrid_p95_stays_within_30ms_of_embeddings_only_p95_at_1000_chunks() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hybrid-ac07").await;

    let provider = MockServer::start().await;
    let embedding = vec![0.1f64; DIMS];
    Mock::given(method("POST"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let inputs = body["input"].as_array().unwrap();
            let data: Vec<serde_json::Value> =
                inputs.iter().map(|_| json!({"embedding": embedding.clone()})).collect();
            ResponseTemplate::new(200).set_body_json(json!({"data": data}))
        })
        .mount(&provider)
        .await;

    let (ct, nonce) = state.secrets.encrypt("test-embed-secret-value").unwrap();
    state.db.upsert_secret(tenant.id, "EMBED_KEY".to_string(), ct, nonce).await.expect("upsert secret");
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

    state
        .db
        .doc_chunks_bulk_insert_with_vectors_for_test(tenant.id, 1000, DIMS)
        .await
        .expect("seed 1k chunks with vectors");

    let mut hybrid_durations = Vec::with_capacity(50);
    for i in 0..50i64 {
        let marker = (i * 19) % 1000;
        let query = format!("wordmarker{marker}");
        let start = std::time::Instant::now();
        let result = docs::doc_search(&state, &tenant, &json!({"query": query, "k": 5}))
            .await
            .expect("hybrid search ok");
        hybrid_durations.push(start.elapsed());
        assert_eq!(result["index"]["mode"], json!("hybrid"), "default mode with a provider must be hybrid: {result:?}");
    }

    let mut embeddings_durations = Vec::with_capacity(50);
    for i in 0..50i64 {
        let marker = (i * 19) % 1000;
        let query = format!("wordmarker{marker}");
        let start = std::time::Instant::now();
        let result = docs::doc_search(&state, &tenant, &json!({"query": query, "k": 5, "mode": "embeddings"}))
            .await
            .expect("embeddings-only search ok");
        embeddings_durations.push(start.elapsed());
        assert_eq!(result["index"]["mode"], json!("embeddings"));
    }

    let hybrid_p95 = p95(hybrid_durations);
    let embeddings_p95 = p95(embeddings_durations);
    let budget = std::time::Duration::from_millis(30);
    assert!(
        hybrid_p95 <= embeddings_p95 + budget,
        "hybrid p95 {hybrid_p95:?} must not exceed embeddings-only p95 {embeddings_p95:?} by more than {budget:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
