//! PRD-mcphost-output-schema-tenant-values-opaque
//! AC2 (P0) -- Given a chain whose step returns
//! `{"rows": [[1, 2]], "n": 3.5, "ok": true}`, When `host.tool.call` and then
//! `host.runs.get` return it, Then both `structuredContent` values validate
//! against the `outputSchema` from `tools/list`.

use crate::common;
use common::{McpClient, TestServer, chain_kind_registry, extract_structured, publish, signup};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

async fn output_schema(client: &McpClient, tool: &str) -> Value {
    let listing = client.tools_list().await.expect("tools/list");
    listing["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == tool)
        .unwrap_or_else(|| panic!("{tool} not listed"))["outputSchema"]
        .clone()
}

fn assert_valid(schema: &Value, instance: &Value, what: &str) {
    let validator = jsonschema::validator_for(schema).expect("outputSchema compiles");
    let errors: Vec<String> = validator.iter_errors(instance).map(|e| format!("{}: {e}", e.instance_path)).collect();
    assert!(errors.is_empty(), "{what} does not validate: {errors:?}\n{instance}");
}

#[tokio::test]
async fn a_chain_step_returning_arrays_floats_and_booleans_validates_on_call_and_runs_get() {
    let server = TestServer::start_with_kinds(chain_kind_registry()).await;
    let (_ns, key) = signup(&server.base_url, "Oschv AC2 Tenant").await;
    let client = McpClient::with_bearer(&server.base_url, &key);

    publish(&client, "emit", "echo", json!({"schema": {"type": "object"}})).await;
    let payload = json!({"rows": [[1, 2]], "n": 3.5, "ok": true});
    publish(&client, "pipeline", "chain", json!({"steps": [{"tool": "emit", "args": payload}]})).await;

    let call_schema = output_schema(&client, "host.tool.call").await;
    let get_schema = output_schema(&client, "host.runs.get").await;

    let sync = client
        .tools_call("host.tool.call", json!({"name": "pipeline", "args": {}}))
        .await
        .expect("sync chain call");
    let sync_structured = extract_structured(&sync);
    let rendered = sync_structured.to_string();
    assert!(rendered.contains("3.5") && rendered.contains("[[1,2]]"), "step result missing from {rendered}");
    assert_valid(&call_schema, &sync_structured, "host.tool.call structuredContent");

    let enqueue = extract_structured(
        &client
            .tools_call("host.tool.call", json!({"name": "pipeline", "args": {}, "async": true}))
            .await
            .expect("enqueue"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();
    let deadline = Instant::now() + Duration::from_secs(15);
    let done = loop {
        let got = client
            .tools_call("host.runs.get", json!({"run_id": run_id}))
            .await
            .expect("runs.get");
        let structured = extract_structured(&got);
        if structured["status"] != json!("queued") && structured["status"] != json!("running") {
            break structured;
        }
        assert!(Instant::now() < deadline, "run never finalized");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(done["result"].to_string().contains("3.5"), "runs.get lost the result: {done}");
    assert_valid(&get_schema, &done, "host.runs.get structuredContent");
}
