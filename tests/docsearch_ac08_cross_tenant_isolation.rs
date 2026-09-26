//! PRD-mcphost-docs-semantic-search
//! AC8 -- Given a query from tenant A, When it runs, Then no chunk of
//! tenant B is ever returned (test seeds identical text in both).

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docsearch-ac08-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn identical_text_in_two_tenants_never_crosses_a_search() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant_a = common::bare_tenant(&state, "docsearch-ac08-a").await;
    let tenant_b = common::bare_tenant(&state, "docsearch-ac08-b").await;

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

    let result_a = docs::doc_search(&state, &tenant_a, &json!({"query": "refunds", "k": 10}))
        .await
        .expect("search a ok");
    let results_a = result_a["results"].as_array().expect("results array");
    assert!(!results_a.is_empty(), "tenant A must find its own document: {result_a:?}");

    let result_b = docs::doc_search(&state, &tenant_b, &json!({"query": "refunds", "k": 10}))
        .await
        .expect("search b ok");
    let results_b = result_b["results"].as_array().expect("results array");
    assert!(!results_b.is_empty(), "tenant B must find its own document: {result_b:?}");

    for hit in results_b {
        assert_eq!(hit["document_id"], doc_b_id, "tenant B's search must never return tenant A's chunk: {hit:?}");
    }
    for hit in results_a {
        assert_ne!(hit["document_id"], doc_b_id, "tenant A's search must never return tenant B's chunk: {hit:?}");
    }

    std::fs::remove_dir_all(&dir).ok();
}
