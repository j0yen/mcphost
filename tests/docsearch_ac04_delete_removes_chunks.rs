//! PRD-mcphost-docs-semantic-search
//! AC4 -- Given a document is deleted, When the indexer runs, Then its
//! chunks are removed and `status.chunks` decreases accordingly.

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docsearch-ac04-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn deleting_a_document_removes_its_chunks_and_status_chunks_decreases() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "docsearch-ac04").await;

    docs::doc_put(
        &state,
        &tenant,
        &json!({"name": "keep.md", "content": "Section one.\n\nThis document stays around."}),
    )
    .await
    .expect("put keep ok");
    let doomed = docs::doc_put(
        &state,
        &tenant,
        &json!({"name": "doomed.md", "content": "Section one.\n\nThis document is about to be deleted."}),
    )
    .await
    .expect("put doomed ok");
    docs_index::tick_once(&state).await.expect("tick 1 ok");

    let status_before = docs::doc_status(&state, &tenant, &json!({})).await.expect("status ok");
    let chunks_before = status_before["index"]["chunks"].as_i64().expect("chunks");
    assert!(chunks_before >= 2, "expected at least one chunk per document: {status_before:?}");

    docs::doc_delete(&state, &tenant, &json!({"id": doomed["id"]})).await.expect("delete ok");
    docs_index::tick_once(&state).await.expect("tick 2 ok");

    let status_after = docs::doc_status(&state, &tenant, &json!({})).await.expect("status ok");
    let chunks_after = status_after["index"]["chunks"].as_i64().expect("chunks");
    assert!(
        chunks_after < chunks_before,
        "status.chunks must decrease after the deleted document's chunks are removed: \
         before={chunks_before} after={chunks_after}"
    );

    let names = docs::doc_search(&state, &tenant, &json!({"query": "deleted", "k": 5}))
        .await
        .expect("search ok");
    let result_names: Vec<String> = names["results"]
        .as_array()
        .expect("results array")
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !result_names.contains(&"doomed.md".to_string()),
        "search must not return chunks of a deleted document: {result_names:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
