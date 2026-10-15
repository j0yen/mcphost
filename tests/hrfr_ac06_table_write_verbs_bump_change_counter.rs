//! PRD-mcphost-index-refresh-on-change
//! AC6 -- Given `TABLE_WRITE_VERBS`, When the test calls each verb on a
//! fixture table, Then `change_counter` increments once per call and the
//! const equals the set of `host.table.*` write tools in the registry.

use crate::common;
use mcphost::tables::{self, TABLE_WRITE_VERBS};
use mcphost::{tables_model, write_verbs};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn scratch_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcphost-hrfr-ac06-{}-{}",
        std::process::id(),
        mcphost::state::now_unix_ms()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn change_counter(dir: &std::path::Path, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(dir.join("mcphost.db")).expect("open db");
    conn.query_row(
        "SELECT change_counter FROM table_change_counters WHERE table_name = ?1",
        [table],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

#[test]
fn the_const_is_exactly_the_host_table_write_verbs_of_the_registry() {
    let registry: BTreeSet<&str> = write_verbs::WRITE_VERBS
        .iter()
        .map(|v| v.name)
        .filter(|n| n.starts_with("host.table."))
        .collect();
    let listed: BTreeSet<&str> = TABLE_WRITE_VERBS.iter().copied().collect();
    assert_eq!(listed.len(), TABLE_WRITE_VERBS.len(), "no verb listed twice");
    assert_eq!(listed, registry, "TABLE_WRITE_VERBS has drifted from the registry's host.table.* write verbs");
}

#[tokio::test]
async fn every_table_write_verb_bumps_the_change_counter_exactly_once() {
    let dir = scratch_dir();
    let state = common::bare_state(&dir).await;
    let tenant = common::bare_tenant(&state, "hrfr-ac06").await;
    let mut exercised: BTreeSet<&str> = BTreeSet::new();

    // host.table.create
    tables::table_create(&state, &tenant, &json!({"name": "fixture", "columns": {"id": "integer", "notes": "text"}}))
        .await
        .expect("create");
    assert_eq!(change_counter(&dir, "fixture"), 1, "create bumps once");
    exercised.insert("host.table.create");

    // host.table.append
    tables::table_append(&state, &tenant, &json!({"table": "fixture", "rows": [{"id": 1, "notes": "a"}, {"id": 2, "notes": "b"}]}))
        .await
        .expect("append");
    assert_eq!(change_counter(&dir, "fixture"), 2, "one append of two rows is one bump");
    exercised.insert("host.table.append");

    // host.table.model_set
    tables_model::model_set(&state, &tenant, &json!({"table": "fixture", "key": "description", "value": "orders"}))
        .await
        .expect("model_set");
    assert_eq!(change_counter(&dir, "fixture"), 3, "model_set bumps once");
    exercised.insert("host.table.model_set");

    // host.table.handle_drop: a handle is a table of its own.
    let summary: Value = tables::table_query(&state, &tenant, &json!({"sql": "SELECT * FROM fixture", "handle": true}), None)
        .await
        .expect("materialize handle");
    let handle = summary["handle"].as_str().expect("handle name").to_string();
    assert_eq!(change_counter(&dir, &handle), 0, "materializing is not a write verb call");
    tables::table_handle_drop(&state, &tenant, &json!({"handle": handle})).await.expect("handle_drop");
    assert_eq!(change_counter(&dir, &handle), 1, "handle_drop bumps once");
    exercised.insert("host.table.handle_drop");

    // host.table.drop
    tables::table_drop(&state, &tenant, &json!({"name": "fixture"})).await.expect("drop");
    assert_eq!(change_counter(&dir, "fixture"), 4, "drop bumps once");
    exercised.insert("host.table.drop");

    // A refused call writes nothing and does not bump.
    tables::table_append(&state, &tenant, &json!({"table": "fixture", "rows": [{"id": 3}]}))
        .await
        .expect_err("the table is gone");
    assert_eq!(change_counter(&dir, "fixture"), 4, "a refused append does not bump");

    let all: BTreeSet<&str> = TABLE_WRITE_VERBS.iter().copied().collect();
    assert_eq!(exercised, all, "this test called every verb in TABLE_WRITE_VERBS");

    std::fs::remove_dir_all(&dir).ok();
}
