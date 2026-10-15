//! PRD-mcphost-docs-telemetry-failed-after-search AC5 (P0) — Given 7 days of
//! rows, When `/status.json` is fetched, Then `docs_gaps.top` has <= 10
//! entries sorted by `failures` desc, each with `query`, `failures`,
//! `top_code`, `top_anchor`, and no entry's `query` contains a digit, a URL
//! or a `t_` token.

use crate::common::TestServer;
use mcphost::state::now_unix;
use serde_json::{Value, json};

#[tokio::test]
async fn dtf_ac05_docs_gaps_top_is_bounded_sorted_and_free_of_identifiers() {
    let server = TestServer::start().await;
    let db = &server.state.db;
    let now = now_unix();
    // 14 distinct failed queries with 1..=14 failures each, spread over the
    // week, plus a decoy older than the window and a never-failed search.
    for q in 1..=14i64 {
        for i in 0..q {
            let query = format!("query {}", (b'a' + q as u8) as char);
            let code = if i % 3 == 0 { "invalid_spec" } else { "chain_mapping_unresolved" };
            db.insert_help_search_event_for_test(Some(1), &query, Some("docs/kinds/chain.md#mappings"), now - 3600 * i, Some(code))
                .await
                .expect("seed");
        }
    }
    // An old-build row that still carries a URL, a namespace and a digit.
    db.insert_help_search_event_for_test(
        Some(1),
        "fetch https://x.y/z for t_8117b3bb 42",
        None,
        now - 60,
        Some("tool_not_found"),
    )
    .await
    .expect("seed raw");
    db.insert_help_search_event_for_test(Some(1), "too old", None, now - 8 * 86_400, Some("invalid_spec"))
        .await
        .expect("seed old");
    db.insert_help_search_event_for_test(Some(1), "never failed", None, now - 10, None).await.expect("seed");

    let status: Value = reqwest::Client::new()
        .get(format!("{}/status.json", server.base_url))
        .send()
        .await
        .expect("GET /status.json")
        .json()
        .await
        .expect("parse");
    let gaps = &status["docs_gaps"];
    assert_eq!(gaps["window_days"], json!(7), "{gaps}");
    let top = gaps["top"].as_array().expect("docs_gaps.top");
    assert!(top.len() <= 10 && top.len() == 10, "{top:?}");
    let failures: Vec<i64> = top.iter().map(|e| e["failures"].as_i64().expect("failures")).collect();
    assert!(failures.windows(2).all(|w| w[0] >= w[1]), "sorted by failures desc: {failures:?}");
    assert_eq!(failures[0], 14, "{failures:?}");
    for e in top {
        for key in ["query", "failures", "top_code", "top_anchor"] {
            assert!(e.get(key).is_some(), "{key} missing: {e}");
        }
        let query = e["query"].as_str().expect("query");
        assert!(!query.chars().any(|c| c.is_ascii_digit()), "digit in {query:?}");
        assert!(!query.contains("http"), "URL in {query:?}");
        assert!(!query.split_whitespace().any(|w| w.starts_with("t_")), "t_ token in {query:?}");
    }
    assert_eq!(top[0]["top_code"], json!("chain_mapping_unresolved"), "{top:?}");
    assert_eq!(top[0]["top_anchor"], json!("docs/kinds/chain.md#mappings"), "{top:?}");
    assert_eq!(gaps["searches"], json!(14 * 15 / 2 + 2), "old row and window excluded: {gaps}");
    assert_eq!(gaps["followed_by_failure"], json!(14 * 15 / 2 + 1), "{gaps}");
}
