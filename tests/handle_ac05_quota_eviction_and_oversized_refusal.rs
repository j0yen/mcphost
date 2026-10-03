//! PRD-mcphost-result-handles
//! AC5 -- Given a free-plan tenant with live handles near its
//! `table_handle_bytes_max`, When a materialisation would push it over,
//! Then the least-recently-queried handles are evicted first until it
//! fits; When a single materialisation alone exceeds the cap, Then
//! `handle_quota_exceeded` names it and nothing is created.
//!
//! Overriding the plan's quota to a small number needs a directly-mutable
//! `AppState.plans` -- `TestServer` hands back an `Arc<AppState>`, which
//! can't be mutated after construction, so this uses `common::bare_state`
//! (an owned `AppState`), the same shape `tables.rs`'s own
//! `table_quota_refuses_past_tables_max` unit test and
//! `docstore_ac06_quota_docs_and_docs_bytes.rs` already use for their own
//! analogous quota overrides.

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

/// A handle of roughly `kib` KiB, built from a single row's
/// `hex(randomblob(..))` text column -- cheap to materialise (SQLite just
/// generates random bytes and hex-encodes them, no row-by-row work)
/// regardless of the byte target.
fn blob_sql_kib(kib: i64) -> String {
    format!("SELECT hex(randomblob({})) AS data", kib * 1024 / 2)
}

#[tokio::test]
async fn eviction_makes_room_least_recently_queried_first() {
    let dir = scratch_dir("evict");
    let mut state = common::bare_state(&dir).await;
    for p in &mut state.plans.plans {
        if p.name == "free" {
            p.table_handle_bytes_max = 1_000_000; // 1 MB, for a fast/deterministic test
        }
    }
    let tenant = common::bare_tenant(&state, "handle-ac05-evict").await;

    // Three ~300 KiB handles (A, B, C), each materialised a beat apart so
    // their created_unix/last_used_unix land in distinct seconds --
    // ORDER BY last_used_unix's own resolution.
    let a = tables::table_query(&state, &tenant, &json!({"sql": blob_sql_kib(300), "handle": true}))
        .await
        .expect("materialize A");
    let a_handle = a["handle"].as_str().unwrap().to_string();
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    let b = tables::table_query(&state, &tenant, &json!({"sql": blob_sql_kib(300), "handle": true}))
        .await
        .expect("materialize B");
    let b_handle = b["handle"].as_str().unwrap().to_string();
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    let c = tables::table_query(&state, &tenant, &json!({"sql": blob_sql_kib(300), "handle": true}))
        .await
        .expect("materialize C");
    let c_handle = c["handle"].as_str().unwrap().to_string();
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    // Touch A (query it) so it's no longer the least-recently-used --
    // requirement 4's eviction order is last_used_unix, not created_unix.
    tables::table_query(&state, &tenant, &json!({"sql": format!("SELECT COUNT(*) FROM {a_handle}")}))
        .await
        .expect("touch A");
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

    // A fourth ~300 KiB handle pushes total live bytes over the 1 MB cap;
    // B (oldest untouched) must be evicted, not A (touched) or C (newest).
    let d = tables::table_query(&state, &tenant, &json!({"sql": blob_sql_kib(300), "handle": true}))
        .await
        .expect("materialize D triggers eviction");
    let d_handle = d["handle"].as_str().unwrap().to_string();

    let handles = tables::table_handles(&state, &tenant, &json!({})).await.expect("host.table.handles");
    let live: Vec<String> = handles["handles"]
        .as_array()
        .expect("handles array")
        .iter()
        .map(|h| h["handle"].as_str().unwrap().to_string())
        .collect();

    assert!(live.contains(&a_handle), "A was touched and must survive: {live:?}");
    assert!(live.contains(&d_handle), "D is the newest materialisation and must survive: {live:?}");
    assert!(
        !live.contains(&b_handle),
        "B was the least-recently-queried live handle and must have been evicted: {live:?}"
    );

    let total_bytes: i64 = handles["handles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["bytes"].as_i64().unwrap())
        .sum();
    assert!(
        total_bytes <= 1_000_000,
        "live handle bytes must fit under the plan's cap after eviction: {total_bytes}"
    );
    let _ = c_handle; // may or may not survive depending on exact byte accounting; not asserted either way

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn single_oversized_materialisation_is_refused_and_creates_nothing() {
    let dir = scratch_dir("oversized");
    let mut state = common::bare_state(&dir).await;
    for p in &mut state.plans.plans {
        if p.name == "free" {
            p.table_handle_bytes_max = 1_000_000; // 1 MB cap
        }
    }
    let tenant = common::bare_tenant(&state, "handle-ac05-oversized").await;

    let err = tables::table_query(&state, &tenant, &json!({"sql": blob_sql_kib(2_000), "handle": true}))
        .await
        .expect_err("a single materialisation over the cap alone must be refused");
    assert_eq!(err.code(), "handle_quota_exceeded", "error: {err:?}");

    let handles = tables::table_handles(&state, &tenant, &json!({})).await.expect("host.table.handles");
    assert!(
        handles["handles"].as_array().unwrap().is_empty(),
        "nothing must have been created: {handles}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
