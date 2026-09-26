//! PRD-mcphost-docs-semantic-search
//! AC1 -- Given three documents put and 10 s elapsed, When
//! `host.docs.status` runs, Then `index.indexed_watermark` equals the
//! documents' watermark, `lag_seconds` is 0, and `chunks` > 0.
//!
//! "10 s elapsed" is the indexer's own real cadence (`docs_index::
//! spawn_scheduler`, started at `mcphost serve`); this test calls
//! `docs_index::tick_once` directly instead of sleeping wall-clock time,
//! the same "expose a single deterministic tick, run it manually" shape
//! `triggers::tick_once`/`retention::prune_sync` already use for their own
//! periodic jobs.

use crate::common;
use mcphost::{docs, docs_index};
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docsearch-ac01-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn status_reports_caught_up_watermark_zero_lag_and_chunks() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "docsearch-ac01").await;

    for (name, content) in [
        ("one.md", "First document.\n\nIt has a couple of paragraphs about nothing much."),
        ("two.md", "Second document.\n\nAlso a couple of paragraphs, different content."),
        ("three.md", "Third document.\n\nEnough text here to produce at least one chunk."),
    ] {
        docs::doc_put(&state, &tenant, &json!({"name": name, "content": content}))
            .await
            .expect("put ok");
    }

    let watermark_before_tick = state.db.documents_watermark(tenant.id).await.expect("watermark");
    assert_eq!(watermark_before_tick, 3);

    docs_index::tick_once(&state).await.expect("tick ok");

    let status = docs::doc_status(&state, &tenant, &json!({})).await.expect("status ok");
    let index = &status["index"];
    assert_eq!(index["indexed_watermark"], json!(watermark_before_tick));
    assert_eq!(index["lag_seconds"], json!(0));
    assert!(index["chunks"].as_i64().unwrap() > 0, "expected chunks > 0, got {index:?}");
    assert_eq!(index["pending_documents"], json!(0));
    assert_eq!(index["mode"], json!("lexical"));

    std::fs::remove_dir_all(&dir).ok();
}
