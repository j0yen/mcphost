//! PRD-mcphost-docs-semantic-search
//! AC6 -- Given embeddings mode and the provider returns 503, When
//! `search` runs, Then results come from the lexical index and the
//! response has `index.mode: "lexical-fallback"`.

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docsearch-ac06-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn provider_outage_degrades_to_lexical_results_not_an_error() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "docsearch-ac06").await;

    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
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

    docs::doc_put(
        &state,
        &tenant,
        &json!({"name": "policy.md", "content": "Section one.\n\nRefunds are processed within fourteen days."}),
    )
    .await
    .expect("put ok");
    // The provider is down during indexing too -- the tick must still
    // succeed and store the chunk lexically (no vector), never error out
    // (non-functional requirement: "a provider outage degrades to lexical
    // answers ... never to an error").
    docs_index::tick_once(&state).await.expect("tick must not fail on a provider outage");

    let status = docs::doc_status(&state, &tenant, &json!({})).await.expect("status ok");
    assert_eq!(status["index"]["mode"], json!("embeddings"), "status still reports the configured mode");

    let result = docs::doc_search(&state, &tenant, &json!({"query": "refunds", "k": 5}))
        .await
        .expect("search must not error even though the provider is down");
    assert_eq!(result["index"]["mode"], json!("lexical-fallback"));
    let results = result["results"].as_array().expect("results array");
    assert!(
        results.iter().any(|r| r["name"] == json!("policy.md")),
        "lexical fallback must still find the document: {results:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
