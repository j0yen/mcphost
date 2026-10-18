//! PRD-mcphost-error-envelope-schema-contract
//! AC5 (P0) -- Given the chain verifier's fixture response from 2026-10-13
//! (`error.data: null`), When validated against the schema, Then it fails, and
//! the same call's new envelope passes.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, extract_structured, publish, signup};
use mcphost::output_schemas::output_schema_for;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const FIXTURE: &str = include_str!("fixtures/errenv/chain_verifier_runs_list_2026-10-13.json");

fn validator() -> jsonschema::Validator {
    let schema = Value::Object((*output_schema_for("host.runs.list").expect("registered")).clone());
    jsonschema::validator_for(&schema).expect("schema compiles")
}

#[test]
fn errenv_ac05_old_response_is_rejected() {
    let old: Value = serde_json::from_str(FIXTURE).expect("fixture json");
    assert_eq!(old["runs"][0]["error"]["data"], Value::Null);
    let errors: Vec<String> = validator().iter_errors(&old).map(|e| e.instance_path.to_string()).collect();
    assert!(
        errors.iter().any(|p| p.ends_with("/error/data")),
        "fixture must fail at error/data, got {errors:?}"
    );
}

#[tokio::test]
async fn errenv_ac05() {
    errenv_ac05_old_response_is_rejected();
    // The same call today: a failed run whose data column is NULL.
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Errenv AC5 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    for i in 0..3 {
        publish(&client, &format!("step{i}"), "echo", json!({"schema": {"type": "object"}})).await;
    }
    let steps: Vec<Value> = (0..3).map(|i| json!({"tool": format!("step{i}"), "args": {}})).collect();
    publish(&client, "pipeline", "chain", json!({"steps": steps})).await;
    let run_id = extract_structured(
        &client
            .tools_call(
                "host.tool_call",
                json!({"name": "pipeline", "args": {}, "async": true, "budget": {"max_child_calls": 2}}),
            )
            .await
            .expect("enqueue"),
    )["run_id"]
        .as_str()
        .expect("run_id")
        .to_string();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let got = extract_structured(&client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("get"));
        if got["status"] == json!("error") {
            break;
        }
        assert!(Instant::now() < deadline, "run never failed: {got}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let conn = rusqlite::Connection::open(server.data_dir.0.join("mcphost.db")).expect("open db");
    conn.execute("UPDATE runs SET error_data_json = NULL WHERE id = ?1", [&run_id]).expect("null column");
    drop(conn);
    let new = extract_structured(&client.tools_call("host.runs.list", json!({})).await.expect("list"));
    assert!(new["runs"].as_array().is_some_and(|r| r.iter().any(|x| x["error"]["kind"] == json!("budget_exceeded"))), "{new}");
    let errors: Vec<String> = validator().iter_errors(&new).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "new envelope must validate: {errors:?}\n{new}");
}
