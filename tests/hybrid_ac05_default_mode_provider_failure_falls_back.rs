//! PRD-mcphost-docs-hybrid-search
//! AC5 -- Given default mode and a provider that fails, When search runs,
//! Then results are the lexical ranking, `index.mode` is
//! `lexical-fallback`, and no error is returned.

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hybrid-ac05-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn default_mode_with_a_failing_provider_degrades_to_lexical_fallback() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hybrid-ac05").await;

    let provider = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(503)).mount(&provider).await;

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

    docs::doc_put(
        &state,
        &tenant,
        &json!({"name": "policy.md", "content": "Refunds are processed within fourteen days."}),
    )
    .await
    .expect("put ok");
    // The provider is down during indexing too -- the tick must still
    // succeed, storing the chunk lexically (no vector).
    docs_index::tick_once(&state).await.expect("tick must not fail on a provider outage");

    let result = docs::doc_search(&state, &tenant, &json!({"query": "refunds", "k": 5}))
        .await
        .expect("search must not error even though the provider is down");
    assert_eq!(result["index"]["mode"], json!("lexical-fallback"));
    let results = result["results"].as_array().expect("results array");
    assert!(
        results.iter().any(|r| r["name"] == json!("policy.md")),
        "lexical fallback must still find the document: {results:?}"
    );
    for r in results {
        assert_eq!(r["ranks"]["embeddings"], serde_json::Value::Null, "a fallback hit has no embeddings rank: {r:?}");
    }

    std::fs::remove_dir_all(&dir).ok();
}
