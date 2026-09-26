//! PRD-mcphost-docs-semantic-search
//! AC3 -- Given a document is updated to version 2 with the phrase
//! removed, When the indexer runs, Then the old chunks are gone, the
//! search no longer returns it, and `indexed_watermark` advanced.

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docsearch-ac03-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

async fn search_names(state: &mcphost::state::AppState, tenant: &mcphost::db::Tenant, query: &str) -> Vec<String> {
    let result = docs::doc_search(state, tenant, &json!({"query": query, "k": 5})).await.expect("search ok");
    result["results"]
        .as_array()
        .expect("results array")
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn version_bump_removing_the_phrase_drops_old_chunks_and_advances_watermark() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "docsearch-ac03").await;

    docs::doc_put(
        &state,
        &tenant,
        &json!({"name": "policy.md", "content": "Section one.\n\nRefunds are processed within fourteen days of purchase."}),
    )
    .await
    .expect("put v1 ok");
    docs_index::tick_once(&state).await.expect("tick 1 ok");

    let watermark_after_v1 = state.db.documents_watermark(tenant.id).await.expect("watermark");
    assert!(
        search_names(&state, &tenant, "refunds").await.contains(&"policy.md".to_string()),
        "v1 must be findable by the phrase it contains"
    );
    let chunks_after_v1 = state.db.doc_chunks_count(tenant.id).await.expect("chunk count");
    assert!(chunks_after_v1 > 0);

    docs::doc_put(
        &state,
        &tenant,
        &json!({"name": "policy.md", "content": "Section one.\n\nAll sales are final, no exceptions apply."}),
    )
    .await
    .expect("put v2 ok");
    docs_index::tick_once(&state).await.expect("tick 2 ok");

    let watermark_after_v2 = state.db.documents_watermark(tenant.id).await.expect("watermark");
    assert!(
        watermark_after_v2 > watermark_after_v1,
        "indexed_watermark must advance past the version-2 update"
    );
    let status = docs::doc_status(&state, &tenant, &json!({})).await.expect("status ok");
    assert_eq!(status["index"]["indexed_watermark"], json!(watermark_after_v2));
    let chunks_after_v2 = state.db.doc_chunks_count(tenant.id).await.expect("chunk count");
    assert_eq!(
        chunks_after_v2, chunks_after_v1,
        "the old version's chunks must be replaced, not accumulated alongside the new ones"
    );

    assert!(
        !search_names(&state, &tenant, "refunds").await.contains(&"policy.md".to_string()),
        "search must no longer return the document once the phrase is gone"
    );
    assert!(
        search_names(&state, &tenant, "sales final").await.contains(&"policy.md".to_string()),
        "the document must still be findable by its new content"
    );

    std::fs::remove_dir_all(&dir).ok();
}
