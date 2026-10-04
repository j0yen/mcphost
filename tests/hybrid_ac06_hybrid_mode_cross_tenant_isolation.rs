//! PRD-mcphost-docs-hybrid-search
//! AC6 -- Given two tenants with overlapping document text, When tenant A
//! searches, Then no hit belongs to tenant B in either candidate list or
//! the fused result.

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hybrid-ac06-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn identical_text_in_two_tenants_never_crosses_a_hybrid_search() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant_a = common::bare_tenant(&state, "hybrid-ac06-a").await;
    let tenant_b = common::bare_tenant(&state, "hybrid-ac06-b").await;

    let provider = MockServer::start().await;
    // Every chunk (and the query) embeds identically -- maximizes the
    // chance a cross-tenant leak would actually surface in the dense
    // candidate list if tenant scoping were ever dropped.
    Mock::given(method("POST"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let inputs = body["input"].as_array().unwrap();
            let data: Vec<serde_json::Value> =
                inputs.iter().map(|_| json!({"embedding": [1.0, 0.0]})).collect();
            ResponseTemplate::new(200).set_body_json(json!({"data": data}))
        })
        .mount(&provider)
        .await;

    for tenant in [&tenant_a, &tenant_b] {
        let (ct, nonce) = state.secrets.encrypt("test-embed-secret-value").unwrap();
        state.db.upsert_secret(tenant.id, "EMBED_KEY".to_string(), ct, nonce).await.expect("upsert secret");
        docs::doc_index_config(
            &state,
            tenant,
            &json!({
                "provider": "openai-compatible",
                "endpoint": format!("{}/v1/embeddings", provider.uri()),
                "model": "m",
                "secret": "EMBED_KEY",
            }),
        )
        .await
        .expect("index_config ok");
    }

    let content = "Section one.\n\nRefunds are processed within fourteen days of purchase.";
    docs::doc_put(&state, &tenant_a, &json!({"name": "policy.md", "content": content}))
        .await
        .expect("put a ok");
    docs::doc_put(&state, &tenant_b, &json!({"name": "policy.md", "content": content}))
        .await
        .expect("put b ok");
    docs_index::tick_once(&state).await.expect("tick ok");

    let doc_a_id = docs::doc_get(&state, &tenant_a, &json!({"name": "policy.md"})).await.unwrap()["id"].clone();
    let doc_b_id = docs::doc_get(&state, &tenant_b, &json!({"name": "policy.md"})).await.unwrap()["id"].clone();
    assert_ne!(doc_a_id, doc_b_id, "sanity: the two tenants' documents have distinct ids");

    let result_a = docs::doc_search(&state, &tenant_a, &json!({"query": "refunds", "k": 10}), None)
        .await
        .expect("search a ok");
    assert_eq!(result_a["index"]["mode"], json!("hybrid"));
    let results_a = result_a["results"].as_array().expect("results array");
    assert!(!results_a.is_empty(), "tenant A must find its own document: {result_a:?}");

    let result_b = docs::doc_search(&state, &tenant_b, &json!({"query": "refunds", "k": 10}), None)
        .await
        .expect("search b ok");
    let results_b = result_b["results"].as_array().expect("results array");
    assert!(!results_b.is_empty(), "tenant B must find its own document: {result_b:?}");

    for hit in results_b {
        assert_eq!(hit["document_id"], doc_b_id, "tenant B's hybrid search must never return tenant A's chunk: {hit:?}");
    }
    for hit in results_a {
        assert_ne!(hit["document_id"], doc_b_id, "tenant A's hybrid search must never return tenant B's chunk: {hit:?}");
    }

    std::fs::remove_dir_all(&dir).ok();
}
