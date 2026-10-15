//! PRD-mcphost-elicitation-with-ask-fallback
//! AC6 (P0) — Given a job-triggered run, When the tool elicits, Then
//! `elicit_unsupported {reason: "no_client"}` is returned and the run row
//! records the code.

use crate::common;
use common::{McpClient, TestServer, extract_structured, poll_until_ready, python_kind_registry, signup};
use mcphost::sandbox;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const TOOL: &str = "import mcphost\ndef main(args):\n    if args.get(\"warm\"):\n        return {\"warm\": True}\n    if args.get(\"catch\"):\n        try:\n            return mcphost.elicit(\"Rotate?\", {\"confirm\": {\"type\": \"boolean\"}})\n        except mcphost.elicit.ElicitError as e:\n            return {\"code\": e.code, \"reason\": e.data.get(\"reason\")}\n    return mcphost.elicit(\"Rotate?\", {\"confirm\": {\"type\": \"boolean\"}})\n";

async fn finished_run(client: &McpClient, args: Value) -> Value {
    let enqueue = extract_structured(
        &client
            .tools_call("host.tool_call", json!({"name": "rotate", "args": args, "async": true}))
            .await
            .expect("enqueue"),
    );
    let run_id = enqueue["run_id"].as_str().expect("run_id").to_string();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let run = extract_structured(&client.tools_call("host.runs.get", json!({"run_id": run_id})).await.expect("runs.get"));
        if matches!(run["status"].as_str(), Some("done" | "error" | "timeout")) || Instant::now() > deadline {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn a_job_run_cannot_elicit_and_its_row_records_elicit_unsupported() {
    if sandbox::require_user_namespaces_or_ci_skip() {
        println!("{} (CI)", sandbox::USERNS_SKIP_MARKER);
        return;
    }
    let envs = common::TempDataDir::new();
    let server = TestServer::start_with_kinds(python_kind_registry(&envs.0)).await;
    let (ns, key) = signup(&server.base_url, "Melc AC6").await;
    let client = McpClient::with_bearer(&server.base_url, &key);
    client
        .tools_call("host.tool_publish", json!({"name": "rotate", "kind": "python", "spec": {"source": TOOL}}))
        .await
        .expect("publish");
    poll_until_ready(&client, &format!("{ns}.rotate"), json!({"warm": true}), Duration::from_secs(20))
        .await
        .expect("warm");

    // The refusal as the tool sees it: code and reason.
    let caught = finished_run(&client, json!({"catch": true})).await;
    assert_eq!(caught["status"], json!("done"), "{caught}");
    let result = &caught["result"];
    assert_eq!(result["code"], json!("elicit_unsupported"), "{caught}");
    assert_eq!(result["reason"], json!("no_client"), "{caught}");

    // Uncaught: the run row itself records the code.
    let failed = finished_run(&client, json!({})).await;
    assert_eq!(failed["status"], json!("error"), "{failed}");
    assert_eq!(failed["error_class"], json!("elicit_unsupported"), "{failed}");
    assert_eq!(
        extract_structured(&client.tools_call("host.ask.list", json!({})).await.expect("list"))["asks"],
        json!([]),
        "a job run must not leave an ask row"
    );
}
