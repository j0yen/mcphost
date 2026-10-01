//! PRD-mcphost-row-policy
//! AC9 (P0) -- Given the 1,000-row fixture, When 100 filtered and 100
//! unfiltered `host.table.query` calls run, Then filtered p95 exceeds
//! unfiltered p95 by at most 3 ms.
//!
//! Calls `tables::table_query` directly against a [`common::bare_state`]
//! (same "the AC is about the policy machinery's own overhead, not HTTP/JSON
//! round-trip jitter" shape `docsearch_ac11_lexical_search_p95_latency.rs`
//! uses) so the measured gap is the row-policy rewrite/audit path's cost,
//! not network noise. Every fixture row satisfies the policy (region "EU"
//! for all 1,000 rows, alice's own attr is "EU"), so both runs return the
//! same 1,000 rows -- the comparison isolates rewrite/audit overhead rather
//! than a smaller filtered result set.

use crate::common;
use mcphost::enduser::{EndUser, EndUserMethod};
use mcphost::tables;
use serde_json::json;

#[tokio::test]
async fn filtered_query_p95_is_within_3ms_of_unfiltered_p95_at_1000_rows() {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-rowpol-ac09-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "rowpol-ac09").await;

    tables::table_create(
        &state,
        &tenant,
        &json!({"name": "orders", "columns": {"id": "integer", "region": "text"}, "primary_key": "id"}),
    )
    .await
    .expect("table create ok");

    let rows: Vec<serde_json::Value> =
        (0..1000i64).map(|i| json!({"id": i, "region": "EU"})).collect();
    tables::table_append(&state, &tenant, &json!({"table": "orders", "rows": rows}))
        .await
        .expect("seed 1000 rows");

    mcphost::rowpolicy::policy_set(
        &state,
        &tenant,
        &json!({
            "target": {"table": "orders"},
            "rule": [{"column_or_attr": "region", "op": "eq", "value": {"attr": "region"}}],
        }),
        None,
    )
    .await
    .expect("policy set ok");
    mcphost::rowpolicy::policy_attrs_set(
        &state,
        &tenant,
        &json!({"subject": "alice", "attrs": {"region": "EU"}}),
        None,
    )
    .await
    .expect("attrs set ok");

    let alice = EndUser {
        subject: "alice".to_string(),
        issuer: None,
        method: EndUserMethod::Assertion,
        verified_at: mcphost::state::now_unix(),
        email: None,
        name: None,
    };

    let query = json!({"sql": "SELECT * FROM orders"});

    let mut unfiltered = Vec::with_capacity(100);
    for _ in 0..100 {
        let start = std::time::Instant::now();
        let result = tables::table_query(&state, &tenant, &query, None).await.expect("unfiltered query ok");
        unfiltered.push(start.elapsed());
        assert_eq!(result["rows"].as_array().expect("rows array").len(), 1000, "{result:?}");
    }

    let mut filtered = Vec::with_capacity(100);
    for _ in 0..100 {
        let start = std::time::Instant::now();
        let result =
            tables::table_query(&state, &tenant, &query, Some(&alice)).await.expect("filtered query ok");
        filtered.push(start.elapsed());
        assert_eq!(result["rows"].as_array().expect("rows array").len(), 1000, "{result:?}");
    }

    unfiltered.sort();
    filtered.sort();
    let unfiltered_p95 = unfiltered[94];
    let filtered_p95 = filtered[94];

    let diff = filtered_p95.saturating_sub(unfiltered_p95);
    assert!(
        diff <= std::time::Duration::from_millis(3),
        "filtered p95 {filtered_p95:?} exceeded unfiltered p95 {unfiltered_p95:?} by {diff:?}, expected <= 3ms"
    );

    std::fs::remove_dir_all(&dir).ok();
}
