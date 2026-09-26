//! PRD-mcphost-docs-semantic-search
//! AC11 -- Given 10 000 chunks, When 100 lexical searches run, Then p95 <
//! 50 ms.
//!
//! Seeded in one transaction via
//! [`mcphost::db::Db::doc_chunks_bulk_insert_for_test`], bypassing
//! `docs::doc_put`/`docs_index::tick_once` entirely -- same "the AC is
//! about lookup latency against a populated index, not about how fast this
//! test can build it" shape
//! `banlist_ac10_cache_lookup_latency_at_scale.rs`'s own
//! `bulk_insert_addr_bans_for_test` already uses.

use crate::common;
use mcphost::docs;
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-docsearch-ac11-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn lexical_search_p95_latency_stays_under_50ms_at_ten_thousand_chunks() {
    let dir = scratch_dir("main");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "docsearch-ac11").await;

    state.db.doc_chunks_bulk_insert_for_test(tenant.id, 10_000).await.expect("seed 10k chunks");

    let mut durations = Vec::with_capacity(100);
    for i in 0..100i64 {
        let marker = (i * 97) % 10_000;
        let query = format!("wordmarker{marker}");
        let start = std::time::Instant::now();
        let result = docs::doc_search(&state, &tenant, &json!({"query": query, "k": 5}))
            .await
            .expect("search ok");
        durations.push(start.elapsed());
        let results = result["results"].as_array().expect("results array");
        assert!(!results.is_empty(), "expected a hit for {query}: {result:?}");
    }

    durations.sort();
    let p95 = durations[94];
    assert!(
        p95 < std::time::Duration::from_millis(50),
        "p95 lexical search latency over 100 queries at 10k chunks was {p95:?}, expected < 50ms"
    );

    std::fs::remove_dir_all(&dir).ok();
}
