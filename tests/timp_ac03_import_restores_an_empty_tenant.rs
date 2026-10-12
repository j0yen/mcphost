//! PRD-mcphost-export-tables-and-import-restore
//! AC3 -- Given an empty tenant B and A's completed export, When
//! `host.import(run_id=…)` completes, Then `host.table.list`,
//! `host.table.describe`, `host.state.list`, `host.docs.list` and the tool
//! catalog (`host.tool_list`) on B match A, and `secrets_to_set` names A's
//! secret.

use crate::common;
use crate::timp;

use common::TestServer;
use serde_json::{Value, json};

/// Drops what legitimately differs per tenant: the clock fields (the same state key is
/// written at a different second) and the `t_<ns>.` namespace prefix on tool names.
fn without_clock(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            Value::Object(m.iter().filter(|(k, _)| *k != "updated_unix").map(|(k, x)| (k.clone(), without_clock(x))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(without_clock).collect()),
        Value::String(n) if n.starts_with("t_") && n.contains('.') => json!(n.split_once('.').map_or(n.as_str(), |(_, rest)| rest)),
        other => other.clone(),
    }
}

#[tokio::test]
async fn import_into_an_empty_tenant_matches_the_source_and_names_the_secret() {
    let server = TestServer::start().await;
    let a = timp::new_tenant(&server, "Timp AC3 A").await;
    a.seed_full().await;
    let (run_id, _entries) = a.export(&server).await;

    let b = timp::new_tenant(&server, "Timp AC3 B").await;
    let done = b.import_run(&run_id).await;
    assert_eq!(done["status"], json!("done"), "import failed: {done}");
    assert_eq!(done["result"]["secrets_to_set"], json!(["api_key"]), "{done}");
    assert_eq!(done["result"]["skipped"], json!([]), "{done}");

    // The docs index rebuilds in the background (chunks_used lags the put), so let B's
    // listing settle to A's before comparing; a hang-guard timeout, not a measurement.
    let (va, vb) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let (va, vb) = (a.views().await, b.views().await);
            if without_clock(&va["docs_list"]) == without_clock(&vb["docs_list"]) {
                return (va, vb);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("B's docs listing never matched A's within 30 s");
    for view in ["table_list", "table_describe", "state_list", "docs_list", "tool_list"] {
        assert_eq!(without_clock(&va[view]), without_clock(&vb[view]), "{view} differs between A and B");
    }
    // Sanity: the views are not vacuously equal.
    assert_eq!(vb["table_list"]["tables"][0]["rows"], json!(3), "{vb}");
    assert_eq!(vb["docs_list"]["documents"][0]["name"], json!("a.md"), "{vb}");
    let rows = b.call("host.table.query", json!({"sql": "SELECT id, name FROM t ORDER BY id"})).await;
    assert_eq!(rows["rows"].as_array().map(Vec::len), Some(3), "{rows}");
}
