//! PRD-mcphost-document-store
//! AC3 -- Given a 2 MiB + 1 byte payload, When `put` runs, Then it is
//! rejected with a size error; at exactly 2 MiB it succeeds.
//!
//! `MCPHOST_DOC_MAX_BYTES` defaults to 2 MiB, well over the unrelated,
//! already-pinned `MAX_REQUEST_BODY_BYTES` (1 MiB, PRD-mcphost-call-limits-
//! honest) transport cap every real `tools/call` HTTP request shares -- so
//! this AC calls `docs::doc_put` directly against a [`common::bare_state`],
//! the same "business logic, not the transport" test shape `tables.rs`'s
//! own module tests already use for quota/schema checks `TestServer`
//! cannot exercise end to end either.

use crate::common;
use mcphost::docs;
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docstore-ac03-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn put_rejects_over_the_cap_and_accepts_exactly_at_it() {
    let dir = scratch_dir("size-cap");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "docstore-ac03").await;

    let max_bytes = 2 * 1024 * 1024usize;
    let over = "a".repeat(max_bytes + 1);
    let err = docs::doc_put(&state, &tenant, &json!({"name": "big.txt", "content": over}))
        .await
        .expect_err("a payload one byte over the cap must be rejected");
    assert_eq!(err.code(), "docs_too_large", "unexpected error: {err:?}");

    let exact = "a".repeat(max_bytes);
    let put = docs::doc_put(&state, &tenant, &json!({"name": "big.txt", "content": exact}))
        .await
        .expect("a payload exactly at the cap must succeed");
    assert_eq!(put["bytes"], json!(max_bytes as i64), "put result: {put:?}");
    assert_eq!(put["version"], json!(1), "put result: {put:?}");

    std::fs::remove_dir_all(&dir).ok();
}
