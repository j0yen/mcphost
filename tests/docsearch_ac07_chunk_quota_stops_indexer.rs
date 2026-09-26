//! PRD-mcphost-docs-semantic-search
//! AC7 -- Given `docs_chunks_max: 100` and documents that would produce
//! 120 chunks, When the indexer runs, Then it stops at 100, `status`
//! reports `quota_chunks_reached: true`, and `search` still answers over
//! the indexed 100.
//!
//! Overriding a plan's quota to a small number for the test needs a
//! directly-mutable `AppState.plans`, same
//! [`common::bare_state`]/[`common::bare_tenant`] shape
//! `docstore_ac06_quota_docs_and_docs_bytes.rs` already uses for its own
//! `docs_max`/`docs_bytes_max` override.

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docsearch-ac07-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// 120 paragraphs, each long enough on its own (~790 bytes) that two of
/// them can never share one ≤800-byte chunk -- exactly 120 chunks once
/// chunked, no merging, no oversized-paragraph splitting.
fn content_producing_120_chunks() -> String {
    let mut paragraphs = Vec::with_capacity(120);
    for i in 0..120 {
        let marker = if i == 0 { "findme-marker ".to_string() } else { String::new() };
        let filler = "x".repeat(790 - marker.len());
        paragraphs.push(format!("{marker}{filler}"));
    }
    paragraphs.join("\n\n")
}

#[tokio::test]
async fn indexer_stops_at_the_chunk_quota_and_search_still_answers() {
    let dir = scratch_dir("main");
    let mut state = common::bare_state(&dir).await;
    for p in &mut state.plans.plans {
        if p.name == "free" {
            p.docs_chunks_max = 100;
        }
    }
    let tenant = common::bare_tenant(&state, "docsearch-ac07").await;

    docs::doc_put(&state, &tenant, &json!({"name": "big.md", "content": content_producing_120_chunks()}))
        .await
        .expect("put ok");
    docs_index::tick_once(&state).await.expect("tick ok");

    let status = docs::doc_status(&state, &tenant, &json!({})).await.expect("status ok");
    assert_eq!(status["index"]["chunks"], json!(100), "indexer must stop exactly at the quota");
    assert_eq!(status["index"]["quota_chunks_reached"], json!(true));
    assert_eq!(status["index"]["pending_documents"], json!(1), "the truncated document stays pending");

    let result = docs::doc_search(&state, &tenant, &json!({"query": "findme marker", "k": 3}))
        .await
        .expect("search ok");
    let results = result["results"].as_array().expect("results array");
    assert!(
        !results.is_empty(),
        "search must still answer over the 100 chunks that did get indexed: {result:?}"
    );

    // Running the tick again must not grow past the quota (same partial
    // set gets recomputed identically, not accumulated).
    docs_index::tick_once(&state).await.expect("tick 2 ok");
    let chunks_after_second_tick = state.db.doc_chunks_count(tenant.id).await.expect("chunk count");
    assert_eq!(chunks_after_second_tick, 100);

    std::fs::remove_dir_all(&dir).ok();
}
