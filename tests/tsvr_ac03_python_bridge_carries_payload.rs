//! PRD-mcphost-table-schema-violation-names-the-remedy
//! AC3 — Given a python tool whose `tool.py` calls `mcphost.table.append`
//! with the five 2026-10-12 row shapes, When each raises
//! `McphostTableError`, Then `err.data` equals the payload the Rust
//! validator built and `str(err)` is the new message, and the same shapes
//! through `tenant_state`'s `validate_row` yield the same payload under
//! code `state_schema_violation`.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::Duration;

const PAYLOAD_KEYS: [&str; 7] = ["field", "kind", "table", "declared", "nearest", "expected", "got"];

/// The five 2026-10-12 shapes: (column declared type in `tables`, row).
fn shapes() -> Vec<Value> {
    vec![
        json!({"op": "sync", "ts": "2026-10-12T00:00:00Z"}),
        json!({"ts": 1760000000}),
        json!({"checked_at": 1760000000}),
        json!({"memory_mb": 12.5}),
        json!({"amount": "abc"}),
    ]
}

fn payload(data: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for k in PAYLOAD_KEYS {
        out.insert(k.to_string(), data[k].clone());
    }
    Value::Object(out)
}

#[tokio::test]
async fn python_error_data_and_message_equal_the_rust_payload() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs_dir = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs_dir.0)).await;
    let (ns, key) = signup(&server.base_url, "TSVR AC3").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call(
            "host.table.create",
            json!({"name": "events", "columns": {
                "ts": "timestamp", "checked_at": "timestamp", "memory_mb": "integer", "amount": "real"
            }}),
        )
        .await
        .expect("table create");

    // What the Rust validator builds, straight over MCP.
    let mut rust = Vec::new();
    for row in shapes() {
        let err = client
            .tools_call("host.table.append", json!({"table": "events", "rows": [row]}))
            .await
            .expect_err("shape must be rejected");
        assert_eq!(err.error_code.as_deref(), Some("table_schema_violation"), "{err:?}");
        rust.push((err.message.clone(), payload(&err.data)));
    }

    let source = r#"import mcphost
def main(args):
    out = []
    for row in args["rows"]:
        try:
            mcphost.table.append(table="events", rows=[row])
            out.append({"raised": False})
        except Exception as e:
            out.append({"cls": type(e).__name__, "code": e.code, "msg": str(e), "data": e.data})
    return {"results": out}
"#;
    client
        .tools_call(
            "host.tool_publish",
            json!({"name": "writer", "kind": "python", "spec": {
                "source": source,
                "args_schema": {"type": "object"},
            }}),
        )
        .await
        .expect("publish");
    let result = poll_until_ready(
        &client,
        &format!("{ns}.writer"),
        json!({"rows": shapes()}),
        Duration::from_secs(15),
    )
    .await
    .unwrap_or_else(|e| panic!("call must succeed: {} {}", e.code, e.message));
    let results = extract_structured(&result)["results"].as_array().expect("results").clone();
    assert_eq!(results.len(), 5);
    for (py, (msg, want)) in results.iter().zip(&rust) {
        assert_eq!(py["cls"], json!("McphostTableError"), "{py}");
        assert_eq!(py["code"], json!("table_schema_violation"), "{py}");
        // PRD-mcphost-table-conflict-structured-error: `str(err)` leads with the code.
        assert_eq!(&py["msg"], &json!(format!("table_schema_violation: {msg}")), "{py}");
        assert_eq!(&payload(&py["data"]), want, "{py}");
    }
}

#[tokio::test]
async fn state_validator_yields_the_same_payload_under_its_own_code() {
    let server = TestServer::start().await;
    let (_ns, key) = signup(&server.base_url, "TSVR AC3 state").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    // host.state has no `timestamp` type: the timestamp-shaped columns are
    // declared `text` there, so only `expected.type` differs for those.
    client
        .tools_call(
            "host.table.create",
            json!({"name": "events", "columns": {
                "ts": "text", "checked_at": "text", "memory_mb": "integer", "amount": "real"
            }}),
        )
        .await
        .expect("table create");
    client
        .tools_call(
            "host.state.table_create",
            json!({"name": "events", "schema": {
                "ts": "text", "checked_at": "text", "memory_mb": "integer", "amount": "real"
            }}),
        )
        .await
        .expect("state table create");
    for row in shapes() {
        let t = client
            .tools_call("host.table.append", json!({"table": "events", "rows": [row.clone()]}))
            .await;
        let s = client
            .tools_call("host.state.insert", json!({"table": "events", "rows": [row.clone()]}))
            .await;
        let (t, s) = match (t, s) {
            (Err(t), Err(s)) => (t, s),
            // `{"ts": ...}` string is valid text on both sides; nothing to compare.
            (Ok(_), Ok(_)) => continue,
            (t, s) => panic!("validators disagree on {row}: {t:?} vs {s:?}"),
        };
        assert_eq!(s.error_code.as_deref(), Some("state_schema_violation"), "{s:?}");
        assert_eq!(s.message, t.message, "{row}");
        assert_eq!(payload(&s.data), payload(&t.data), "{row}");
    }
    // Declared timestamp column: the shared constructor is the same
    // function, so the timestamp-shaped payload is covered by AC2's drift
    // test and the python test above.
}
