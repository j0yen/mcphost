//! PRD-mcphost-docs-semantic-search
//! AC10 -- Given `reindex {}` on a tenant with 5 documents, When the
//! indexer runs, Then all five are re-chunked and `indexed_watermark`
//! ends at the current watermark.

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docsearch-ac10-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn reindex_all_rechunks_every_document_and_watermark_catches_up() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "docsearch-ac10").await;

    for i in 0..5 {
        docs::doc_put(
            &state,
            &tenant,
            &json!({"name": format!("doc{i}.md"), "content": format!("Section one.\n\nDocument number {i} body text.")}),
        )
        .await
        .unwrap_or_else(|e| panic!("put doc{i} failed: {e:?}"));
    }
    docs_index::tick_once(&state).await.expect("initial tick ok");

    let watermark = state.db.documents_watermark(tenant.id).await.expect("watermark");
    assert_eq!(watermark, 5);
    let status_before = docs::doc_status(&state, &tenant, &json!({})).await.expect("status ok");
    assert_eq!(status_before["index"]["indexed_watermark"], json!(5));
    assert_eq!(status_before["index"]["pending_documents"], json!(0));
    let chunks_before = status_before["index"]["chunks"].as_i64().expect("chunks");
    assert!(chunks_before >= 5);

    docs::doc_reindex(&state, &tenant, &json!({})).await.expect("reindex ok");

    let status_after_reindex_call = docs::doc_status(&state, &tenant, &json!({})).await.expect("status ok");
    assert_eq!(
        status_after_reindex_call["index"]["indexed_watermark"], json!(0),
        "reindex {{}} must rewind the watermark to 0 so every document is pending again"
    );
    assert_eq!(status_after_reindex_call["index"]["pending_documents"], json!(5));
    assert_eq!(status_after_reindex_call["index"]["rebuilding"], json!(true));

    docs_index::tick_once(&state).await.expect("post-reindex tick ok");

    let status_after = docs::doc_status(&state, &tenant, &json!({})).await.expect("status ok");
    assert_eq!(
        status_after["index"]["indexed_watermark"], json!(watermark),
        "indexed_watermark must end back at the current watermark after re-chunking all five"
    );
    assert_eq!(status_after["index"]["pending_documents"], json!(0));
    assert_eq!(status_after["index"]["rebuilding"], json!(false));
    assert_eq!(
        status_after["index"]["chunks"], json!(chunks_before),
        "re-chunking identical content must land on the same chunk count, not accumulate"
    );

    std::fs::remove_dir_all(&dir).ok();
}
