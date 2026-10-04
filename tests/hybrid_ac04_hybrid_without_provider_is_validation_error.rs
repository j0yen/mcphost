//! PRD-mcphost-docs-hybrid-search
//! AC4 -- Given `mode: "hybrid"` on a tenant with no provider, When search
//! runs, Then a validation error names the missing provider config and no
//! results are returned.

use crate::common;
use mcphost::docs;
use mcphost::errors::AppError;
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hybrid-ac04-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn hybrid_mode_without_a_configured_provider_is_a_validation_error() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hybrid-ac04").await;

    docs::doc_put(&state, &tenant, &json!({"name": "policy.md", "content": "Refunds within 14 days."}))
        .await
        .expect("put ok");

    let err = docs::doc_search(&state, &tenant, &json!({"query": "refunds", "k": 5, "mode": "hybrid"}), None)
        .await
        .expect_err("hybrid mode without a provider must be refused, not return results");

    match &err {
        AppError::InvalidArgs(msg) => {
            assert!(msg.contains("provider"), "error must name the missing provider config: {msg}");
        }
        other => panic!("expected AppError::InvalidArgs naming the missing provider, got {other:?}"),
    }

    // Same refusal for `mode: "embeddings"` -- requirement 4 names both.
    let err2 = docs::doc_search(&state, &tenant, &json!({"query": "refunds", "k": 5, "mode": "embeddings"}), None)
        .await
        .expect_err("embeddings mode without a provider must also be refused");
    assert!(matches!(err2, AppError::InvalidArgs(_)));

    std::fs::remove_dir_all(&dir).ok();
}
