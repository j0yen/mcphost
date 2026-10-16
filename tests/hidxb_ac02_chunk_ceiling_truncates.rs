//! PRD-mcphost-index-create-from-sources-b
//! AC2 -- Given a table source whose text would exceed `index_chunks_max`,
//! When the tick runs, Then the chunk count equals the ceiling,
//! `last_error: "quota_index_chunks_max"` is set, and the docs pass of the
//! same tick still completes.

use crate::common;
use mcphost::{docs, docs_index, index, tables};
use serde_json::json;

#[tokio::test]
async fn a_table_past_index_chunks_max_truncates_at_the_ceiling_and_docs_still_index() {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hidxb-ac02-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let mut state = common::bare_state(&dir).await;
    // A tiny ceiling on the free plan so the test needs a handful of rows,
    // not 20 000 chunks.
    let ceiling = 5;
    for plan in state.plans.plans.iter_mut().filter(|p| p.name == "free") {
        plan.index_chunks_max = ceiling;
    }
    let tenant = common::bare_tenant(&state, "hidxb-ac02").await;

    tables::table_create(&state, &tenant, &json!({"name": "notes", "columns": {"id": "integer", "body": "text"}}))
        .await
        .expect("table create");
    let rows: Vec<_> = (0..20).map(|i| json!({"id": i, "body": format!("row {i} says hello")})).collect();
    tables::table_append(&state, &tenant, &json!({"table": "notes", "rows": rows})).await.expect("append");
    docs::doc_put(&state, &tenant, &json!({"name": "d.md", "content": "a plain document about gift wrap"}))
        .await
        .expect("put ok");

    index::index_create(
        &state,
        &tenant,
        &json!({"name": "big", "sources": [{"kind": "table", "table": "notes", "columns": ["body"]}]}),
    )
    .await
    .expect("create ok");

    docs_index::tick_once(&state).await.expect("tick");

    let status = index::index_status(&state, &tenant, &json!({"name": "big"})).await.expect("status");
    assert_eq!(status["chunks"], json!(ceiling), "{status}");
    assert_eq!(status["sources"][0]["last_error"], json!("quota_index_chunks_max"), "{status}");
    assert_eq!(status["pending"], json!(0), "a truncated source is finished, not retried: {status}");

    let found = docs::doc_search(&state, &tenant, &json!({"query": "gift wrap"}), None).await.expect("search");
    assert!(
        !found["results"].as_array().expect("results").is_empty(),
        "the docs pass of the same tick completed: {found}"
    );
    std::fs::remove_dir_all(&dir).ok();
}
