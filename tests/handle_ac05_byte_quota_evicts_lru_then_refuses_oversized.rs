//! PRD-mcphost-result-handles
//! AC5 -- Given a tenant with live handles near its plan's
//! `table_handle_bytes_max`, When a materialisation would push it over,
//! Then the least-recently-queried handles are evicted until it fits;
//! When a single materialisation alone exceeds the cap, Then
//! `handle_quota_exceeded` names the cap and nothing is created.
//!
//! Overriding the quota to a small, exactly-computable number needs a
//! directly-mutable `AppState.plans` -- same `common::bare_state`/
//! `bare_tenant` shape `docstore_ac06`'s own `docs_max`/`docs_bytes_max`
//! override uses, calling `tables::table_query` directly rather than over
//! HTTP so the handle-bytes estimate (`LENGTH(CAST(col AS TEXT))` summed
//! per row -- see `handles.rs`'s module doc) stays exactly predictable:
//! each row's `v` column is exactly 50 characters, so an N-row handle
//! estimates to `50 * N` bytes.

use crate::common;
use mcphost::tables;
use serde_json::json;

fn scratch_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-handle-ac05-{label}-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[tokio::test]
async fn evicts_lru_to_fit_then_refuses_a_single_oversized_handle() {
    let dir = scratch_dir("quota");
    let mut state = common::bare_state(&dir).await;
    for p in &mut state.plans.plans {
        if p.name == "free" {
            p.table_handle_bytes_max = 900;
        }
    }
    let tenant = common::bare_tenant(&state, "handle-ac05").await;

    tables::table_create(&state, &tenant, &json!({"name": "big", "columns": {"v": "text"}}))
        .await
        .expect("create big");
    let v50 = "x".repeat(50);
    let rows: Vec<_> = (0..20).map(|_| json!({"v": v50})).collect();
    tables::table_append(&state, &tenant, &json!({"table": "big", "rows": rows}))
        .await
        .expect("append 20 rows of 50 chars each");

    // handle_a: 10 rows * 50 bytes = 500 bytes -- fits alone.
    let handle_a = tables::table_query(&state, &tenant, &json!({"sql": "SELECT v FROM big LIMIT 10", "handle": true}))
        .await
        .expect("handle_a materializes");
    assert_eq!(handle_a["bytes"], 500, "handle_a: {handle_a}");
    let name_a = handle_a["handle"].as_str().expect("handle_a name").to_string();

    // handle_b: another 500 bytes -- 500 + 500 = 1000 > 900, so handle_a
    // (the only, hence least-recently-queried, live handle) is evicted to
    // make room before handle_b is inserted.
    let handle_b = tables::table_query(
        &state,
        &tenant,
        &json!({"sql": "SELECT v FROM big LIMIT 10 OFFSET 10", "handle": true}),
    )
    .await
    .expect("handle_b materializes, evicting handle_a");
    assert_eq!(handle_b["bytes"], 500, "handle_b: {handle_b}");
    let name_b = handle_b["handle"].as_str().expect("handle_b name").to_string();

    let listed = mcphost::handles::handles_list(&state, &tenant, &json!({})).await.expect("handles list");
    let handles: Vec<&str> = listed["handles"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|h| h["handle"].as_str())
        .collect();
    assert_eq!(handles, vec![name_b.as_str()], "handle_a must have been evicted: {listed}");
    assert_eq!(listed["bytes_used"], 500, "listed: {listed}");

    let err_a = tables::table_query(&state, &tenant, &json!({"sql": format!("SELECT * FROM {name_a}")}))
        .await
        .unwrap_err();
    assert_eq!(err_a.code(), "handle_not_found", "err_a: {err_a:?}");

    // A single materialisation whose own bytes (20 * 50 = 1000) alone
    // exceed the 900-byte cap is refused, and creates nothing -- handle_b
    // is still the only live handle afterward.
    let err_oversized = tables::table_query(&state, &tenant, &json!({"sql": "SELECT v FROM big", "handle": true}))
        .await
        .unwrap_err();
    assert_eq!(err_oversized.code(), "handle_quota_exceeded", "err: {err_oversized:?}");
    if let mcphost::errors::AppError::Structured { data, .. } = &err_oversized {
        assert_eq!(data["limit"]["value"], 900, "err data: {data}");
    } else {
        panic!("expected AppError::Structured, got {err_oversized:?}");
    }

    let listed_after = mcphost::handles::handles_list(&state, &tenant, &json!({})).await.expect("handles list");
    let handles_after: Vec<&str> = listed_after["handles"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|h| h["handle"].as_str())
        .collect();
    assert_eq!(handles_after, vec![name_b.as_str()], "nothing created over quota: {listed_after}");

    std::fs::remove_dir_all(&dir).ok();
}
