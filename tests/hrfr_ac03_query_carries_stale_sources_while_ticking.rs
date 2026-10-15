//! PRD-mcphost-index-refresh-on-change
//! AC3 -- Given the tick in progress, When `host.index.query` runs, Then
//! `index.stale_sources` lists the moving source with `since_s >= 0`; after
//! the tick the list is empty.

use crate::common;
use mcphost::{docs_index, index, tables};
use serde_json::json;

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hrfr-ac03-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn rows(range: std::ops::Range<i64>) -> Vec<serde_json::Value> {
    range.map(|i| json!({"id": i, "notes": format!("order {i} wants gift wrap")})).collect()
}

#[tokio::test]
async fn a_query_during_a_multi_tick_refresh_lists_the_moving_source_and_the_list_empties_after() {
    let dir = scratch_dir();
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hrfr-ac03").await;

    tables::table_create(&state, &tenant, &json!({"name": "orders", "columns": {"id": "integer", "notes": "text"}}))
        .await
        .expect("table create");
    tables::table_append(&state, &tenant, &json!({"table": "orders", "rows": rows(0..10)})).await.expect("append");
    index::index_create(
        &state,
        &tenant,
        &json!({"name": "handbook", "sources": [{"kind": "table", "table": "orders", "columns": ["notes"]}]}),
    )
    .await
    .expect("create ok");
    docs_index::tick_once(&state).await.expect("build tick");

    let q = json!({"name": "handbook", "query": "gift wrap"});
    let idle = index::index_query(&state, &tenant, &q).await.expect("query");
    assert_eq!(idle["index"]["stale_sources"], json!([]), "a current index lists nothing: {idle}");
    assert_eq!(idle["results"].as_array().unwrap().len(), 5, "k defaults to 5: {idle}");

    // 120 rows are three ticks of 50: the first tick leaves the source moving.
    tables::table_append(&state, &tenant, &json!({"table": "orders", "rows": rows(10..130)})).await.expect("append");
    docs_index::tick_once(&state).await.expect("tick in progress");

    let moving = index::index_query(&state, &tenant, &q).await.expect("query");
    let stale = moving["index"]["stale_sources"].as_array().expect("stale_sources");
    assert_eq!(stale.len(), 1, "{moving}");
    assert_eq!(stale[0]["ref"], json!("orders"));
    assert_eq!(stale[0]["behind_by"], json!(70), "120 appended, 50 read: {moving}");
    assert!(stale[0]["since_s"].as_i64().unwrap() >= 0, "{moving}");

    docs_index::tick_once(&state).await.expect("tick 2");
    docs_index::tick_once(&state).await.expect("tick 3");

    let done = index::index_query(&state, &tenant, &q).await.expect("query");
    assert_eq!(done["index"]["stale_sources"], json!([]), "{done}");
    assert_eq!(done["index"]["chunks"], json!(130), "{done}");

    std::fs::remove_dir_all(&dir).ok();
}
