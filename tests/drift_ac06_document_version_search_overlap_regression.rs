//! PRD-mcphost-drift-review
//! AC6 -- Given a document uploaded as version 3 and 5 logged searches
//! whose top hits included version 2, When reviewed, Then 5 search
//! deltas exist and any with top-5 overlap under 3 counts as regressed.

use crate::common;
use mcphost::{docs, docs_index, drift, tables_model};
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-drift-ac06-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn document_v3_regresses_5_searches_that_depended_on_v2() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "drift-ac06").await;

    docs::doc_put(&state, &tenant, &json!({"name": "policy.md", "content": "version one has nothing special"}))
        .await
        .expect("put v1");
    docs_index::tick_once(&state).await.expect("index tick v1");

    docs::doc_put(
        &state,
        &tenant,
        &json!({"name": "policy.md", "content": "version two mentions zzzkeyword right here"}),
    )
    .await
    .expect("put v2");
    docs_index::tick_once(&state).await.expect("index tick v2");

    for _ in 0..5 {
        let result = docs::doc_search(
            &state,
            &tenant,
            &json!({"query": "zzzkeyword", "mode": "lexical", "k": 5}),
            None,
        )
        .await
        .expect("search v2");
        let results = result["results"].as_array().expect("results array");
        assert!(!results.is_empty(), "v2 search must hit policy.md: {result}");
    }

    docs::doc_put(
        &state,
        &tenant,
        &json!({"name": "policy.md", "content": "version three removed the special term entirely"}),
    )
    .await
    .expect("put v3");
    docs_index::tick_once(&state).await.expect("index tick v3");

    tables_model::tick_once(&state).await.expect("tick_once");

    let reviews = drift::reviews(&state, &tenant, &json!({})).await.expect("reviews");
    let summary = reviews["reviews"]
        .as_array()
        .expect("reviews array")
        .iter()
        .find(|r| r["target"] == "policy.md" && r["kind"] == "document")
        .unwrap_or_else(|| panic!("no document review for policy.md: {reviews}"))
        .clone();
    assert_eq!(summary["new_version"], 3, "review: {summary}");
    let item_id = summary["item_id"].as_str().expect("item_id").to_string();

    let review = drift::review_item(&state, &tenant, &json!({"item_id": item_id})).await.expect("review");
    let deltas = review["deltas"].as_array().expect("deltas array");
    assert_eq!(deltas.len(), 5, "expected 5 search deltas: {review}");
    for delta in deltas {
        let overlap = delta["overlap"].as_i64().expect("overlap");
        assert_eq!(delta["regressed"], json!(overlap < 3), "delta: {delta}");
    }
    assert!(
        deltas.iter().all(|d| d["regressed"] == json!(true)),
        "every delta should be regressed once 'zzzkeyword' no longer matches: {review}"
    );
    assert_eq!(review["regressed_count"], 5, "review: {review}");
}
