//! PRD-mcphost-docs-hybrid-search
//! AC3 -- Given `mode: "lexical"`, When search runs, Then the provider is
//! not called (the test's fake provider records zero requests) and
//! `index.mode` is `lexical`.

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hybrid-ac03-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn explicit_lexical_mode_never_calls_the_provider() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hybrid-ac03").await;

    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&provider)
        .await;

    // Index lexically first -- while provider is still "none" -- so the
    // FTS5 index is populated without ever touching the provider.
    docs::doc_put(
        &state,
        &tenant,
        &json!({"name": "policy.md", "content": "Refunds are processed within fourteen days."}),
    )
    .await
    .expect("put ok");
    docs_index::tick_once(&state).await.expect("tick ok");

    // Configure a provider AFTER indexing, and never tick again -- proves
    // mode: "lexical" alone decides whether the provider is called, not
    // whether one happens to be configured.
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

    let result = docs::doc_search(&state, &tenant, &json!({"query": "refunds", "k": 5, "mode": "lexical"}), None)
        .await
        .expect("search ok");
    assert_eq!(result["index"]["mode"], json!("lexical"));
    let results = result["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "lexical search must still find the indexed chunk: {result:?}");

    let received = provider.received_requests().await.expect("requests recorded");
    assert_eq!(received.len(), 0, "mode: lexical must never call the embeddings provider: {received:?}");

    std::fs::remove_dir_all(&dir).ok();
}
