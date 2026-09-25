//! PRD-mcphost-document-store
//! AC6 -- Given plan `docs_max: 2`, When a third document is put, Then
//! `quota_docs` returns; given `docs_bytes_max` exceeded, `quota_docs_bytes`
//! returns.
//!
//! Overriding a plan's quota to a small number for the test needs a
//! directly-mutable `AppState.plans` -- `TestServer` hands back an
//! `Arc<AppState>`, which can't be mutated after construction, so this uses
//! [`common::bare_state`] (an owned `AppState`), the same shape
//! `tables.rs`'s own `table_quota_refuses_past_tables_max` unit test uses
//! for its analogous `table_tables_max` override.

use crate::common;
use mcphost::docs;
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docstore-ac06-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn third_document_over_docs_max_is_refused() {
    let dir = scratch_dir("docs-max");
    let mut state = common::bare_state(&dir).await;
    for p in &mut state.plans.plans {
        if p.name == "free" {
            p.docs_max = 2;
        }
    }
    let tenant = common::bare_tenant(&state, "docstore-ac06-max").await;

    docs::doc_put(&state, &tenant, &json!({"name": "one.md", "content": "one"}))
        .await
        .expect("first doc ok");
    docs::doc_put(&state, &tenant, &json!({"name": "two.md", "content": "two"}))
        .await
        .expect("second doc ok");
    let err = docs::doc_put(&state, &tenant, &json!({"name": "three.md", "content": "three"}))
        .await
        .expect_err("a third document over docs_max must be refused");
    assert_eq!(err.code(), "quota_docs", "unexpected error: {err:?}");

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn put_over_docs_bytes_max_is_refused() {
    let dir = scratch_dir("docs-bytes-max");
    let mut state = common::bare_state(&dir).await;
    for p in &mut state.plans.plans {
        if p.name == "free" {
            p.docs_bytes_max = 10;
        }
    }
    let tenant = common::bare_tenant(&state, "docstore-ac06-bytes").await;

    let err = docs::doc_put(
        &state,
        &tenant,
        &json!({"name": "big.md", "content": "this is more than ten bytes"}),
    )
    .await
    .expect_err("a put over docs_bytes_max must be refused");
    assert_eq!(err.code(), "quota_docs_bytes", "unexpected error: {err:?}");

    std::fs::remove_dir_all(&dir).ok();
}
