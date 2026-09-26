//! PRD-mcphost-docs-semantic-search
//! AC5 -- Given `index_config {provider: "openai-compatible", endpoint:
//! <test server>, model: "m", secret: "EMBED_KEY"}`, When the rebuild
//! completes, Then `status.index.mode` is `embeddings`, the test server
//! received one embeddings request per chunk batch with the bearer from
//! the tenant secret, and `search` ranks by cosine.

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docsearch-ac05-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A deterministic fake embedder: any input mentioning "apple" gets
/// `[1.0, 0.0]`, "banana" gets `[0.0, 1.0]`, anything else `[0.5, 0.5]` --
/// enough to make cosine ranking meaningfully distinguish the two
/// documents below without a real model.
fn fake_embed_response(req: &Request) -> ResponseTemplate {
    let body: serde_json::Value = serde_json::from_slice(&req.body).expect("valid JSON body");
    let inputs = body["input"].as_array().expect("input array");
    let data: Vec<serde_json::Value> = inputs
        .iter()
        .map(|v| {
            let text = v.as_str().unwrap_or("");
            let embedding: Vec<f64> = if text.contains("apple") {
                vec![1.0, 0.0]
            } else if text.contains("banana") {
                vec![0.0, 1.0]
            } else {
                vec![0.5, 0.5]
            };
            json!({"embedding": embedding})
        })
        .collect();
    ResponseTemplate::new(200).set_body_json(json!({"data": data}))
}

#[tokio::test]
async fn rebuild_switches_to_embeddings_mode_and_search_ranks_by_cosine() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "docsearch-ac05").await;

    let provider = MockServer::start().await;
    Mock::given(method("POST")).respond_with(fake_embed_response).mount(&provider).await;

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

    docs::doc_put(&state, &tenant, &json!({"name": "apple.md", "content": "apple apple apple"}))
        .await
        .expect("put apple ok");
    docs::doc_put(&state, &tenant, &json!({"name": "banana.md", "content": "banana banana banana"}))
        .await
        .expect("put banana ok");
    docs_index::tick_once(&state).await.expect("tick ok");

    let status = docs::doc_status(&state, &tenant, &json!({})).await.expect("status ok");
    assert_eq!(status["index"]["mode"], json!("embeddings"));
    assert_eq!(status["index"]["rebuilding"], json!(false), "rebuild must be complete: {status:?}");
    assert_eq!(status["index"]["pending_documents"], json!(0));

    let result = docs::doc_search(&state, &tenant, &json!({"query": "apple", "k": 5}))
        .await
        .expect("search ok");
    assert_eq!(result["index"]["mode"], json!("embeddings"));
    let results = result["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "expected results: {result:?}");
    assert_eq!(
        results[0]["name"], json!("apple.md"),
        "cosine must rank the apple document above the banana one for an \"apple\" query: {results:?}"
    );

    // Two indexing requests (one per document's own chunk batch) plus one
    // search-time query embedding request.
    let received = provider.received_requests().await.expect("requests recorded");
    assert_eq!(received.len(), 3, "expected one embeddings request per chunk batch plus one for the query: {received:?}");
    for req in &received {
        let auth = req.headers.get("authorization").expect("authorization header").to_str().expect("ascii header");
        assert_eq!(auth, "Bearer test-embed-secret-value");
    }

    std::fs::remove_dir_all(&dir).ok();
}
